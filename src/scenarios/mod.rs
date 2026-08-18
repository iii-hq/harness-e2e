use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{bail, Result};
use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assessment::{AssessmentKind, AssessmentPolicy, AssessmentSource};
use crate::context::E2eContext;
use crate::report::HardGateReport;
use crate::wire::SessionMetricsResponse;

mod assessment;
pub mod build;
pub mod cognition;
pub mod common;
pub mod coordination;
pub mod custom_validator;
pub mod deliverable;
pub mod direct_answer;
mod domain;
mod kit;
pub mod mechanical_reaction;
pub mod multi_subagent_validation;
pub mod orchestration;
pub mod persistent_state;
pub mod pr_review_regressions;
mod probe;
pub mod reactive_automation;
pub mod receiving_operation;
pub mod reliability;
pub mod research_pipeline;
pub mod security_review;
pub mod shell_coder_sandbox;
pub mod subagent_validation;
pub mod subagent_validation_failure;
pub mod timer_wake;
pub mod validation_chain;
pub mod validation_loop;
pub mod validation_scope_enforcement;
pub mod validation_self_repair;

pub use domain::{
    stable_seed, ArtifactExpectation, CapturedDeliverable, CapturedDeliverableContent,
    CapturedInvariant, ComplexityProfile, ComplexityTier, DeliverableContract, InvariantSpec,
    ProvenanceEvidence, ScenarioCase, WorkExpectation,
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
    pub source: AssessmentSource,
}

impl CriterionSpec {
    pub const fn required_deterministic(
        id: &'static str,
        weight: u8,
        description: &'static str,
        dimension: crate::report::EvaluationDimension,
    ) -> Self {
        Self {
            id,
            weight,
            description,
            kind: AssessmentKind::RequiredCheck,
            policy: AssessmentPolicy::HardGate,
            dimension,
            source: AssessmentSource::Deterministic,
        }
    }

    pub const fn advisory_deterministic(
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
            source: AssessmentSource::Deterministic,
        }
    }

    pub const fn advisory_judge(id: &'static str, weight: u8, description: &'static str) -> Self {
        Self {
            id,
            weight,
            description,
            kind: AssessmentKind::Signal,
            policy: AssessmentPolicy::Advisory,
            dimension: crate::report::EvaluationDimension::StructuralIntegrity,
            source: AssessmentSource::Judge,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionPolicy {
    pub max_turns: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    pub max_total_tokens: u64,
    /// Stop only after this many seconds without observable useful progress.
    /// Large scenarios have no fixed wall-clock deadline.
    pub stuck_timeout_seconds: u64,
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
        if self.max_total_tokens == 0 {
            bail!("scenario '{scenario_id}': execution.max_total_tokens=0; expected at least 1");
        }
        if self.stuck_timeout_seconds == 0 {
            bail!(
                "scenario '{scenario_id}': execution.stuck_timeout_seconds=0; expected at least 1"
            );
        }
        if self
            .max_output_tokens
            .is_some_and(|max_output_tokens| self.max_total_tokens < max_output_tokens)
        {
            let max_output_tokens = self.max_output_tokens.expect("checked above");
            bail!(
                "scenario '{scenario_id}': execution.max_total_tokens={} is lower than execution.max_output_tokens={max_output_tokens}; expected max_total_tokens >= max_output_tokens",
                self.max_total_tokens
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
    pub judge_reference: Option<Value>,
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
        if self.case.deliverable_contract.artifacts.is_empty() != self.capture.is_none() {
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
            if criterion.kind == AssessmentKind::AssetQuality
                || criterion.kind == AssessmentKind::AssetValidation
                || criterion.source == AssessmentSource::AssetAnalyzer
            {
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
            if criterion.source != AssessmentSource::Deterministic
                && criterion.policy != AssessmentPolicy::Advisory
            {
                bail!(
                    "scenario '{}': AI-derived criterion '{}' must remain advisory",
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

    pub fn needs_judge(&self) -> bool {
        self.judge_reference.is_some()
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
    pub hard_gates: Vec<HardGateReport>,
    pub awards: Vec<CriterionAward>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioExecutionKind {
    HarnessTurn,
    CompositeFlow,
}

pub struct CriterionAward {
    pub id: String,
    pub awarded: u8,
    pub reason: String,
}

fn captured_gate_invariants(objective: ObjectiveEvaluation) -> Vec<CapturedInvariant> {
    objective
        .hard_gates
        .into_iter()
        .map(|gate| CapturedInvariant {
            id: gate.id,
            passed: gate.passed,
            reason: gate.reason,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioSuite {
    /// The scenarios every standing gate runs.
    Canonical,
    /// Extended coverage: reliability regressions, graph and loop
    /// engineering, build-shaped deliverables, and context handling. Opt in
    /// per run; the canonical gate cost stays unchanged.
    Extended,
}

impl ScenarioSuite {
    pub fn scenarios(self) -> Vec<ScenarioId> {
        ScenarioId::ALL
            .into_iter()
            .filter(|scenario| scenario.suite() == self && !scenario.manual_cli_only())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioId {
    #[value(name = "direct_answer")]
    DirectAnswer,
    #[value(name = "persistent_state")]
    PersistentState,
    #[value(name = "reactive_automation")]
    ReactiveAutomation,
    #[value(name = "shell_coder_sandbox")]
    ShellCoderSandbox,
    #[value(name = "research_pipeline")]
    ResearchPipeline,
    #[value(name = "security_review")]
    SecurityReview,
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
    #[value(name = "multi_subagent_validation")]
    MultiSubagentValidation,
    #[value(name = "subagent_validation_failure")]
    SubagentValidationFailure,
    #[value(name = "custom_validator")]
    CustomValidator,
    #[value(name = "validation_self_repair")]
    ValidationSelfRepair,
    #[value(name = "validation_scope_enforcement")]
    ValidationScopeEnforcement,
    #[value(name = "validation_chain")]
    ValidationChain,
    #[serde(rename = "coordination.1")]
    #[value(name = "coordination.1")]
    Coordination1,
    #[serde(rename = "coordination.2")]
    #[value(name = "coordination.2")]
    Coordination2,
    #[serde(rename = "coordination.3")]
    #[value(name = "coordination.3")]
    Coordination3,
    #[serde(rename = "coordination.4")]
    #[value(name = "coordination.4")]
    Coordination4,
    #[serde(rename = "coordination.5")]
    #[value(name = "coordination.5")]
    Coordination5,
    #[serde(rename = "pr_review.token_takeover")]
    #[value(name = "pr_review.token_takeover")]
    PrReviewTokenTakeover,
    #[serde(rename = "pr_review.reconnect_sweep")]
    #[value(name = "pr_review.reconnect_sweep")]
    PrReviewReconnectSweep,
    #[serde(rename = "pr_review.asset_retry_ack")]
    #[value(name = "pr_review.asset_retry_ack")]
    PrReviewAssetRetryAck,
    #[serde(rename = "pr_review.presence_reconnect")]
    #[value(name = "pr_review.presence_reconnect")]
    PrReviewPresenceReconnect,
    #[serde(rename = "pr_review.prompt_provenance")]
    #[value(name = "pr_review.prompt_provenance")]
    PrReviewPromptProvenance,
    #[serde(rename = "build.security_scanner")]
    #[value(name = "build.security_scanner")]
    BuildSecurityScanner,
    #[serde(rename = "cognition.goal_drift")]
    #[value(name = "cognition.goal_drift")]
    CognitionGoalDrift,
    #[serde(rename = "cognition.injection_resistance")]
    #[value(name = "cognition.injection_resistance")]
    CognitionInjectionResistance,
    #[serde(rename = "cognition.instruction_precedence")]
    #[value(name = "cognition.instruction_precedence")]
    CognitionInstructionPrecedence,
    #[serde(rename = "cognition.stale_memory_refresh")]
    #[value(name = "cognition.stale_memory_refresh")]
    CognitionStaleMemoryRefresh,
    #[serde(rename = "cognition.subagent_context_handoff")]
    #[value(name = "cognition.subagent_context_handoff")]
    CognitionSubagentContextHandoff,
    #[serde(rename = "cognition.subagent_scope")]
    #[value(name = "cognition.subagent_scope")]
    CognitionSubagentScope,
    #[serde(rename = "deliverable.anomaly_report")]
    #[value(name = "deliverable.anomaly_report")]
    DeliverableAnomalyReport,
    #[serde(rename = "deliverable.api_contract")]
    #[value(name = "deliverable.api_contract")]
    DeliverableApiContract,
    #[serde(rename = "deliverable.architecture_diagram")]
    #[value(name = "deliverable.architecture_diagram")]
    DeliverableArchitectureDiagram,
    #[serde(rename = "deliverable.game_simulation")]
    #[value(name = "deliverable.game_simulation")]
    DeliverableGameSimulation,
    #[serde(rename = "deliverable.payload_fidelity")]
    #[value(name = "deliverable.payload_fidelity")]
    DeliverablePayloadFidelity,
    #[serde(rename = "deliverable.scene_graph")]
    #[value(name = "deliverable.scene_graph")]
    DeliverableSceneGraph,
    #[serde(rename = "deliverable.static_site")]
    #[value(name = "deliverable.static_site")]
    DeliverableStaticSite,
    #[serde(rename = "deliverable.svg_chart")]
    #[value(name = "deliverable.svg_chart")]
    DeliverableSvgChart,
    #[serde(rename = "deliverable.world_bible")]
    #[value(name = "deliverable.world_bible")]
    DeliverableWorldBible,
    #[serde(rename = "orchestration.checkpoint_resume")]
    #[value(name = "orchestration.checkpoint_resume")]
    OrchestrationCheckpointResume,
    #[serde(rename = "orchestration.cycle_refusal")]
    #[value(name = "orchestration.cycle_refusal")]
    OrchestrationCycleRefusal,
    #[serde(rename = "orchestration.diamond_merge")]
    #[value(name = "orchestration.diamond_merge")]
    OrchestrationDiamondMerge,
    #[serde(rename = "orchestration.exact_iteration_budget")]
    #[value(name = "orchestration.exact_iteration_budget")]
    OrchestrationExactIterationBudget,
    #[serde(rename = "orchestration.fanout_join")]
    #[value(name = "orchestration.fanout_join")]
    OrchestrationFanoutJoin,
    #[serde(rename = "orchestration.impossible_stop")]
    #[value(name = "orchestration.impossible_stop")]
    OrchestrationImpossibleStop,
    #[serde(rename = "orchestration.repair_convergence")]
    #[value(name = "orchestration.repair_convergence")]
    OrchestrationRepairConvergence,
    #[serde(rename = "orchestration.topological_order")]
    #[value(name = "orchestration.topological_order")]
    OrchestrationTopologicalOrder,
    #[serde(rename = "reliability.amplification_bound")]
    #[value(name = "reliability.amplification_bound")]
    ReliabilityAmplificationBound,
    #[serde(rename = "reliability.binding_hygiene")]
    #[value(name = "reliability.binding_hygiene")]
    ReliabilityBindingHygiene,
    #[serde(rename = "reliability.idempotent_apply")]
    #[value(name = "reliability.idempotent_apply")]
    ReliabilityIdempotentApply,
    #[serde(rename = "reliability.missing_function")]
    #[value(name = "reliability.missing_function")]
    ReliabilityMissingFunction,
    #[serde(rename = "reliability.permanent_stop")]
    #[value(name = "reliability.permanent_stop")]
    ReliabilityPermanentStop,
    #[serde(rename = "reliability.stale_counter")]
    #[value(name = "reliability.stale_counter")]
    ReliabilityStaleCounter,
    #[serde(rename = "reliability.transient_recovery")]
    #[value(name = "reliability.transient_recovery")]
    ReliabilityTransientRecovery,
    #[serde(rename = "reliability.vanishing_function")]
    #[value(name = "reliability.vanishing_function")]
    ReliabilityVanishingFunction,
}

impl ScenarioId {
    pub const ALL: [Self; 59] = [
        Self::DirectAnswer,
        Self::PersistentState,
        Self::ReactiveAutomation,
        Self::ShellCoderSandbox,
        Self::ResearchPipeline,
        Self::SecurityReview,
        Self::MechanicalReaction,
        Self::TimerWake,
        Self::ReceivingOperation,
        Self::ValidationLoop,
        Self::SubagentValidation,
        Self::MultiSubagentValidation,
        Self::SubagentValidationFailure,
        Self::CustomValidator,
        Self::ValidationSelfRepair,
        Self::ValidationScopeEnforcement,
        Self::ValidationChain,
        Self::Coordination1,
        Self::Coordination2,
        Self::Coordination3,
        Self::Coordination4,
        Self::Coordination5,
        Self::PrReviewTokenTakeover,
        Self::PrReviewReconnectSweep,
        Self::PrReviewAssetRetryAck,
        Self::PrReviewPresenceReconnect,
        Self::PrReviewPromptProvenance,
        Self::BuildSecurityScanner,
        Self::CognitionGoalDrift,
        Self::CognitionInjectionResistance,
        Self::CognitionInstructionPrecedence,
        Self::CognitionStaleMemoryRefresh,
        Self::CognitionSubagentContextHandoff,
        Self::CognitionSubagentScope,
        Self::DeliverableAnomalyReport,
        Self::DeliverableApiContract,
        Self::DeliverableArchitectureDiagram,
        Self::DeliverableGameSimulation,
        Self::DeliverablePayloadFidelity,
        Self::DeliverableSceneGraph,
        Self::DeliverableStaticSite,
        Self::DeliverableSvgChart,
        Self::DeliverableWorldBible,
        Self::OrchestrationCheckpointResume,
        Self::OrchestrationCycleRefusal,
        Self::OrchestrationDiamondMerge,
        Self::OrchestrationExactIterationBudget,
        Self::OrchestrationFanoutJoin,
        Self::OrchestrationImpossibleStop,
        Self::OrchestrationRepairConvergence,
        Self::OrchestrationTopologicalOrder,
        Self::ReliabilityAmplificationBound,
        Self::ReliabilityBindingHygiene,
        Self::ReliabilityIdempotentApply,
        Self::ReliabilityMissingFunction,
        Self::ReliabilityPermanentStop,
        Self::ReliabilityStaleCounter,
        Self::ReliabilityTransientRecovery,
        Self::ReliabilityVanishingFunction,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectAnswer => direct_answer::ID,
            Self::PersistentState => persistent_state::ID,
            Self::ReactiveAutomation => reactive_automation::ID,
            Self::ShellCoderSandbox => shell_coder_sandbox::ID,
            Self::ResearchPipeline => research_pipeline::ID,
            Self::SecurityReview => security_review::ID,
            Self::MechanicalReaction => mechanical_reaction::ID,
            Self::TimerWake => timer_wake::ID,
            Self::ReceivingOperation => receiving_operation::ID,
            Self::ValidationLoop => validation_loop::ID,
            Self::SubagentValidation => subagent_validation::ID,
            Self::MultiSubagentValidation => multi_subagent_validation::ID,
            Self::SubagentValidationFailure => subagent_validation_failure::ID,
            Self::CustomValidator => custom_validator::ID,
            Self::ValidationSelfRepair => validation_self_repair::ID,
            Self::ValidationScopeEnforcement => validation_scope_enforcement::ID,
            Self::ValidationChain => validation_chain::ID,
            Self::Coordination1 => coordination::ID_1,
            Self::Coordination2 => coordination::ID_2,
            Self::Coordination3 => coordination::ID_3,
            Self::Coordination4 => coordination::ID_4,
            Self::Coordination5 => coordination::ID_5,
            Self::PrReviewTokenTakeover => pr_review_regressions::TOKEN_TAKEOVER_ID,
            Self::PrReviewReconnectSweep => pr_review_regressions::RECONNECT_SWEEP_ID,
            Self::PrReviewAssetRetryAck => pr_review_regressions::ASSET_RETRY_ACK_ID,
            Self::PrReviewPresenceReconnect => pr_review_regressions::PRESENCE_RECONNECT_ID,
            Self::PrReviewPromptProvenance => pr_review_regressions::PROMPT_PROVENANCE_ID,
            Self::BuildSecurityScanner => build::security_scanner::ID,
            Self::CognitionGoalDrift => cognition::goal_drift::ID,
            Self::CognitionInjectionResistance => cognition::injection_resistance::ID,
            Self::CognitionInstructionPrecedence => cognition::instruction_precedence::ID,
            Self::CognitionStaleMemoryRefresh => cognition::stale_memory_refresh::ID,
            Self::CognitionSubagentContextHandoff => cognition::subagent_context_handoff::ID,
            Self::CognitionSubagentScope => cognition::subagent_scope::ID,
            Self::DeliverableAnomalyReport => deliverable::anomaly_report::ID,
            Self::DeliverableApiContract => deliverable::api_contract::ID,
            Self::DeliverableArchitectureDiagram => deliverable::architecture_diagram::ID,
            Self::DeliverableGameSimulation => deliverable::game_simulation::ID,
            Self::DeliverablePayloadFidelity => deliverable::payload_fidelity::ID,
            Self::DeliverableSceneGraph => deliverable::scene_graph::ID,
            Self::DeliverableStaticSite => deliverable::static_site::ID,
            Self::DeliverableSvgChart => deliverable::svg_chart::ID,
            Self::DeliverableWorldBible => deliverable::world_bible::ID,
            Self::OrchestrationCheckpointResume => orchestration::checkpoint_resume::ID,
            Self::OrchestrationCycleRefusal => orchestration::cycle_refusal::ID,
            Self::OrchestrationDiamondMerge => orchestration::diamond_merge::ID,
            Self::OrchestrationExactIterationBudget => orchestration::exact_iteration_budget::ID,
            Self::OrchestrationFanoutJoin => orchestration::fanout_join::ID,
            Self::OrchestrationImpossibleStop => orchestration::impossible_stop::ID,
            Self::OrchestrationRepairConvergence => orchestration::repair_convergence::ID,
            Self::OrchestrationTopologicalOrder => orchestration::topological_order::ID,
            Self::ReliabilityAmplificationBound => reliability::amplification_bound::ID,
            Self::ReliabilityBindingHygiene => reliability::binding_hygiene::ID,
            Self::ReliabilityIdempotentApply => reliability::idempotent_apply::ID,
            Self::ReliabilityMissingFunction => reliability::missing_function::ID,
            Self::ReliabilityPermanentStop => reliability::permanent_stop::ID,
            Self::ReliabilityStaleCounter => reliability::stale_counter::ID,
            Self::ReliabilityTransientRecovery => reliability::transient_recovery::ID,
            Self::ReliabilityVanishingFunction => reliability::vanishing_function::ID,
        }
    }

    pub fn spec(self, run_id: &str) -> ScenarioSpec {
        match self {
            Self::DirectAnswer => direct_answer::scenario(run_id),
            Self::PersistentState => persistent_state::scenario(run_id),
            Self::ReactiveAutomation => reactive_automation::scenario(run_id),
            Self::ShellCoderSandbox => shell_coder_sandbox::scenario(run_id),
            Self::ResearchPipeline => research_pipeline::scenario(run_id),
            Self::SecurityReview => security_review::scenario(run_id),
            Self::MechanicalReaction => mechanical_reaction::scenario(run_id),
            Self::TimerWake => timer_wake::scenario(run_id),
            Self::ReceivingOperation => receiving_operation::scenario(run_id),
            Self::ValidationLoop => validation_loop::scenario(run_id),
            Self::SubagentValidation => subagent_validation::scenario(run_id),
            Self::MultiSubagentValidation => multi_subagent_validation::scenario(run_id),
            Self::SubagentValidationFailure => subagent_validation_failure::scenario(run_id),
            Self::CustomValidator => custom_validator::scenario(run_id),
            Self::ValidationSelfRepair => validation_self_repair::scenario(run_id),
            Self::ValidationScopeEnforcement => validation_scope_enforcement::scenario(run_id),
            Self::ValidationChain => validation_chain::scenario(run_id),
            Self::Coordination1 => coordination::scenario(coordination::Rung::One, run_id),
            Self::Coordination2 => coordination::scenario(coordination::Rung::Two, run_id),
            Self::Coordination3 => coordination::scenario(coordination::Rung::Three, run_id),
            Self::Coordination4 => coordination::scenario(coordination::Rung::Four, run_id),
            Self::Coordination5 => coordination::scenario(coordination::Rung::Five, run_id),
            Self::PrReviewTokenTakeover => pr_review_regressions::scenario(
                pr_review_regressions::ReviewCase::TokenTakeover,
                run_id,
            ),
            Self::PrReviewReconnectSweep => pr_review_regressions::scenario(
                pr_review_regressions::ReviewCase::ReconnectSweep,
                run_id,
            ),
            Self::PrReviewAssetRetryAck => pr_review_regressions::scenario(
                pr_review_regressions::ReviewCase::AssetRetryAck,
                run_id,
            ),
            Self::PrReviewPresenceReconnect => pr_review_regressions::scenario(
                pr_review_regressions::ReviewCase::PresenceReconnect,
                run_id,
            ),
            Self::PrReviewPromptProvenance => pr_review_regressions::scenario(
                pr_review_regressions::ReviewCase::PromptProvenance,
                run_id,
            ),
            Self::BuildSecurityScanner => build::security_scanner::scenario(run_id),
            Self::CognitionGoalDrift => cognition::goal_drift::scenario(run_id),
            Self::CognitionInjectionResistance => cognition::injection_resistance::scenario(run_id),
            Self::CognitionInstructionPrecedence => {
                cognition::instruction_precedence::scenario(run_id)
            }
            Self::CognitionStaleMemoryRefresh => cognition::stale_memory_refresh::scenario(run_id),
            Self::CognitionSubagentContextHandoff => {
                cognition::subagent_context_handoff::scenario(run_id)
            }
            Self::CognitionSubagentScope => cognition::subagent_scope::scenario(run_id),
            Self::DeliverableAnomalyReport => deliverable::anomaly_report::scenario(run_id),
            Self::DeliverableApiContract => deliverable::api_contract::scenario(run_id),
            Self::DeliverableArchitectureDiagram => {
                deliverable::architecture_diagram::scenario(run_id)
            }
            Self::DeliverableGameSimulation => deliverable::game_simulation::scenario(run_id),
            Self::DeliverablePayloadFidelity => deliverable::payload_fidelity::scenario(run_id),
            Self::DeliverableSceneGraph => deliverable::scene_graph::scenario(run_id),
            Self::DeliverableStaticSite => deliverable::static_site::scenario(run_id),
            Self::DeliverableSvgChart => deliverable::svg_chart::scenario(run_id),
            Self::DeliverableWorldBible => deliverable::world_bible::scenario(run_id),
            Self::OrchestrationCheckpointResume => {
                orchestration::checkpoint_resume::scenario(run_id)
            }
            Self::OrchestrationCycleRefusal => orchestration::cycle_refusal::scenario(run_id),
            Self::OrchestrationDiamondMerge => orchestration::diamond_merge::scenario(run_id),
            Self::OrchestrationExactIterationBudget => {
                orchestration::exact_iteration_budget::scenario(run_id)
            }
            Self::OrchestrationFanoutJoin => orchestration::fanout_join::scenario(run_id),
            Self::OrchestrationImpossibleStop => orchestration::impossible_stop::scenario(run_id),
            Self::OrchestrationRepairConvergence => {
                orchestration::repair_convergence::scenario(run_id)
            }
            Self::OrchestrationTopologicalOrder => {
                orchestration::topological_order::scenario(run_id)
            }
            Self::ReliabilityAmplificationBound => {
                reliability::amplification_bound::scenario(run_id)
            }
            Self::ReliabilityBindingHygiene => reliability::binding_hygiene::scenario(run_id),
            Self::ReliabilityIdempotentApply => reliability::idempotent_apply::scenario(run_id),
            Self::ReliabilityMissingFunction => reliability::missing_function::scenario(run_id),
            Self::ReliabilityPermanentStop => reliability::permanent_stop::scenario(run_id),
            Self::ReliabilityStaleCounter => reliability::stale_counter::scenario(run_id),
            Self::ReliabilityTransientRecovery => reliability::transient_recovery::scenario(run_id),
            Self::ReliabilityVanishingFunction => reliability::vanishing_function::scenario(run_id),
        }
    }

    pub fn materialize(self, namespace: &str, seed: u64) -> Result<MaterializedScenario> {
        let materialized = match self {
            Self::DirectAnswer => direct_answer::materialize(namespace, seed)?,
            Self::PersistentState => persistent_state::materialize(namespace, seed)?,
            Self::ReactiveAutomation => reactive_automation::materialize(namespace, seed)?,
            Self::ShellCoderSandbox => shell_coder_sandbox::materialize(namespace, seed)?,
            Self::ResearchPipeline => research_pipeline::materialize(namespace, seed)?,
            Self::SecurityReview => security_review::materialize(namespace, seed)?,
            Self::MechanicalReaction => mechanical_reaction::materialize(namespace, seed)?,
            Self::TimerWake => timer_wake::materialize(namespace, seed)?,
            Self::ReceivingOperation => receiving_operation::materialize(namespace, seed)?,
            Self::SubagentValidation => subagent_validation::materialize(namespace, seed)?,
            Self::MultiSubagentValidation => {
                multi_subagent_validation::materialize(namespace, seed)?
            }
            Self::SubagentValidationFailure => {
                subagent_validation_failure::materialize(namespace, seed)?
            }
            Self::ValidationLoop => validation_loop::materialize(namespace, seed)?,
            Self::CustomValidator => custom_validator::materialize(namespace, seed)?,
            Self::ValidationSelfRepair => validation_self_repair::materialize(namespace, seed)?,
            Self::ValidationScopeEnforcement => {
                validation_scope_enforcement::materialize(namespace, seed)?
            }
            Self::ValidationChain => validation_chain::materialize(namespace, seed)?,
            Self::Coordination1 => {
                coordination::materialize(coordination::Rung::One, namespace, seed)?
            }
            Self::Coordination2 => {
                coordination::materialize(coordination::Rung::Two, namespace, seed)?
            }
            Self::Coordination3 => {
                coordination::materialize(coordination::Rung::Three, namespace, seed)?
            }
            Self::Coordination4 => {
                coordination::materialize(coordination::Rung::Four, namespace, seed)?
            }
            Self::Coordination5 => {
                coordination::materialize(coordination::Rung::Five, namespace, seed)?
            }
            Self::PrReviewTokenTakeover => pr_review_regressions::materialize(
                pr_review_regressions::ReviewCase::TokenTakeover,
                namespace,
                seed,
            )?,
            Self::PrReviewReconnectSweep => pr_review_regressions::materialize(
                pr_review_regressions::ReviewCase::ReconnectSweep,
                namespace,
                seed,
            )?,
            Self::PrReviewAssetRetryAck => pr_review_regressions::materialize(
                pr_review_regressions::ReviewCase::AssetRetryAck,
                namespace,
                seed,
            )?,
            Self::PrReviewPresenceReconnect => pr_review_regressions::materialize(
                pr_review_regressions::ReviewCase::PresenceReconnect,
                namespace,
                seed,
            )?,
            Self::PrReviewPromptProvenance => pr_review_regressions::materialize(
                pr_review_regressions::ReviewCase::PromptProvenance,
                namespace,
                seed,
            )?,
            Self::BuildSecurityScanner => build::security_scanner::materialize(namespace, seed)?,
            Self::CognitionGoalDrift => cognition::goal_drift::materialize(namespace, seed)?,
            Self::CognitionInjectionResistance => {
                cognition::injection_resistance::materialize(namespace, seed)?
            }
            Self::CognitionInstructionPrecedence => {
                cognition::instruction_precedence::materialize(namespace, seed)?
            }
            Self::CognitionStaleMemoryRefresh => {
                cognition::stale_memory_refresh::materialize(namespace, seed)?
            }
            Self::CognitionSubagentContextHandoff => {
                cognition::subagent_context_handoff::materialize(namespace, seed)?
            }
            Self::CognitionSubagentScope => {
                cognition::subagent_scope::materialize(namespace, seed)?
            }
            Self::DeliverableAnomalyReport => {
                deliverable::anomaly_report::materialize(namespace, seed)?
            }
            Self::DeliverableApiContract => {
                deliverable::api_contract::materialize(namespace, seed)?
            }
            Self::DeliverableArchitectureDiagram => {
                deliverable::architecture_diagram::materialize(namespace, seed)?
            }
            Self::DeliverableGameSimulation => {
                deliverable::game_simulation::materialize(namespace, seed)?
            }
            Self::DeliverablePayloadFidelity => {
                deliverable::payload_fidelity::materialize(namespace, seed)?
            }
            Self::DeliverableSceneGraph => deliverable::scene_graph::materialize(namespace, seed)?,
            Self::DeliverableStaticSite => deliverable::static_site::materialize(namespace, seed)?,
            Self::DeliverableSvgChart => deliverable::svg_chart::materialize(namespace, seed)?,
            Self::DeliverableWorldBible => deliverable::world_bible::materialize(namespace, seed)?,
            Self::OrchestrationCheckpointResume => {
                orchestration::checkpoint_resume::materialize(namespace, seed)?
            }
            Self::OrchestrationCycleRefusal => {
                orchestration::cycle_refusal::materialize(namespace, seed)?
            }
            Self::OrchestrationDiamondMerge => {
                orchestration::diamond_merge::materialize(namespace, seed)?
            }
            Self::OrchestrationExactIterationBudget => {
                orchestration::exact_iteration_budget::materialize(namespace, seed)?
            }
            Self::OrchestrationFanoutJoin => {
                orchestration::fanout_join::materialize(namespace, seed)?
            }
            Self::OrchestrationImpossibleStop => {
                orchestration::impossible_stop::materialize(namespace, seed)?
            }
            Self::OrchestrationRepairConvergence => {
                orchestration::repair_convergence::materialize(namespace, seed)?
            }
            Self::OrchestrationTopologicalOrder => {
                orchestration::topological_order::materialize(namespace, seed)?
            }
            Self::ReliabilityAmplificationBound => {
                reliability::amplification_bound::materialize(namespace, seed)?
            }
            Self::ReliabilityBindingHygiene => {
                reliability::binding_hygiene::materialize(namespace, seed)?
            }
            Self::ReliabilityIdempotentApply => {
                reliability::idempotent_apply::materialize(namespace, seed)?
            }
            Self::ReliabilityMissingFunction => {
                reliability::missing_function::materialize(namespace, seed)?
            }
            Self::ReliabilityPermanentStop => {
                reliability::permanent_stop::materialize(namespace, seed)?
            }
            Self::ReliabilityStaleCounter => {
                reliability::stale_counter::materialize(namespace, seed)?
            }
            Self::ReliabilityTransientRecovery => {
                reliability::transient_recovery::materialize(namespace, seed)?
            }
            Self::ReliabilityVanishingFunction => {
                reliability::vanishing_function::materialize(namespace, seed)?
            }
        };
        materialized.validate()?;
        Ok(materialized)
    }

    pub fn canonical_seed(self) -> u64 {
        // Stable FNV-1a keeps canonical cases reproducible without tying their
        // identity to a particular execution or retry attempt.
        stable_seed(self.as_str())
    }

    pub fn execution_kind(self) -> ScenarioExecutionKind {
        match self {
            Self::SecurityReview => ScenarioExecutionKind::CompositeFlow,
            _ => ScenarioExecutionKind::HarnessTurn,
        }
    }

    /// Which suite this scenario belongs to. The four extended families are
    /// identified by their id prefix so a new scenario joins its suite by
    /// name alone.
    pub fn suite(self) -> ScenarioSuite {
        match self.as_str().split('.').next() {
            Some("build" | "cognition" | "deliverable" | "orchestration" | "reliability") => {
                ScenarioSuite::Extended
            }
            _ => ScenarioSuite::Canonical,
        }
    }

    pub fn manual_cli_only(self) -> bool {
        matches!(self, Self::SecurityReview)
    }
}

pub fn selected(requested: &[ScenarioId]) -> Vec<ScenarioId> {
    selected_in(requested, ScenarioSuite::Canonical)
}

/// Explicit ids win; an empty selection falls back to the whole suite.
pub fn selected_in(requested: &[ScenarioId], suite: ScenarioSuite) -> Vec<ScenarioId> {
    if requested.is_empty() {
        return suite.scenarios();
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
    fn registry_contains_fifty_seven_unique_valid_scenarios() {
        let mut ids = HashSet::new();
        for scenario in ScenarioId::ALL {
            assert!(ids.insert(scenario.as_str()));
            scenario.spec("run").validate().unwrap();
            scenario
                .materialize("run", scenario.canonical_seed())
                .unwrap();
        }
        assert_eq!(ids.len(), 59);
    }

    #[test]
    fn the_extended_suite_holds_the_four_new_families() {
        let extended = ScenarioSuite::Extended.scenarios();
        assert_eq!(extended.len(), 32);
        for scenario in &extended {
            let family = scenario.as_str().split('.').next().unwrap_or_default();
            assert!(matches!(
                family,
                "build" | "cognition" | "deliverable" | "orchestration" | "reliability"
            ));
        }
    }

    #[test]
    fn an_empty_selection_never_reaches_beyond_the_canonical_suite() {
        let canonical = selected(&[]);
        assert_eq!(canonical.len(), ScenarioSuite::Canonical.scenarios().len());
        assert!(canonical
            .iter()
            .all(|scenario| scenario.suite() == ScenarioSuite::Canonical));
        assert_eq!(
            canonical.len() + ScenarioSuite::Extended.scenarios().len() + 1,
            ScenarioId::ALL.len()
        );
    }

    #[test]
    fn an_extended_scenario_is_still_selectable_by_id() {
        let requested = [ScenarioId::ReliabilityStaleCounter];
        assert_eq!(selected(&requested), requested);
        assert_eq!(
            selected_in(&[], ScenarioSuite::Extended),
            ScenarioSuite::Extended.scenarios()
        );
    }

    #[test]
    fn explicit_selection_preserves_order_and_deduplicates() {
        assert_eq!(
            selected(&[
                ScenarioId::ReactiveAutomation,
                ScenarioId::DirectAnswer,
                ScenarioId::ReactiveAutomation,
            ]),
            vec![ScenarioId::ReactiveAutomation, ScenarioId::DirectAnswer]
        );
    }

    #[test]
    fn default_selection_excludes_manually_prepared_scenarios() {
        let selected = selected(&[]);
        assert!(!selected.contains(&ScenarioId::SecurityReview));
        assert!(!selected.contains(&ScenarioId::DeliverableSceneGraph));
    }

    #[test]
    fn materialized_cases_are_stable_across_attempt_namespaces() {
        let first = ScenarioId::PersistentState
            .materialize("attempt-a", 42)
            .unwrap();
        let retry = ScenarioId::PersistentState
            .materialize("attempt-b", 42)
            .unwrap();
        let other_seed = ScenarioId::PersistentState
            .materialize("attempt-c", 43)
            .unwrap();

        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.spec.prompt, retry.spec.prompt);
        assert_ne!(first.case.case_id, other_seed.case.case_id);
        assert_ne!(first.case.inputs, other_seed.case.inputs);
        assert_eq!(first.case.scenario_version, 4);
    }

    #[test]
    fn converted_scenarios_publish_expected_complexity_tiers_and_contracts() {
        let state = ScenarioId::PersistentState.materialize("state", 7).unwrap();
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
            ScenarioId::ReactiveAutomation,
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
            ScenarioId::MultiSubagentValidation,
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
            let first = scenario.materialize("attempt-a", 313).unwrap();
            let retry = scenario.materialize("attempt-b", 313).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id, "{scenario:?}");
            assert_eq!(first.case.inputs, retry.case.inputs, "{scenario:?}");
            assert_eq!(
                first.case.inputs_sha256, retry.case.inputs_sha256,
                "{scenario:?}"
            );

            if scenario == ScenarioId::DirectAnswer {
                assert_eq!(first.case.complexity.tier, domain::ComplexityTier::L0Atomic);
                assert!(first.case.deliverable_contract.artifacts.is_empty());
                assert!(first.capture.is_none());
                continue;
            }

            if scenario.execution_kind() == ScenarioExecutionKind::CompositeFlow {
                assert!(first.case.deliverable_contract.artifacts.is_empty());
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
        let mut spec = ScenarioId::PersistentState.spec("run");
        spec.version = 0;

        assert_eq!(
            spec.validate().unwrap_err().to_string(),
            "scenario 'persistent_state': version=0; expected version >= 1"
        );
    }

    #[test]
    fn validation_identifies_the_invalid_execution_field() {
        type ValidationCase = (&'static str, fn(&mut ExecutionPolicy), &'static str);

        let cases: [ValidationCase; 5] = [
            (
                "max_turns",
                |execution| execution.max_turns = 0,
                "scenario 'persistent_state': execution.max_turns=0; expected at least 1",
            ),
            (
                "max_output_tokens",
                |execution| execution.max_output_tokens = Some(0),
                "scenario 'persistent_state': execution.max_output_tokens=0; expected None (provider limit) or at least 1",
            ),
            (
                "max_total_tokens",
                |execution| execution.max_total_tokens = 0,
                "scenario 'persistent_state': execution.max_total_tokens=0; expected at least 1",
            ),
            (
                "stuck_timeout_seconds",
                |execution| execution.stuck_timeout_seconds = 0,
                "scenario 'persistent_state': execution.stuck_timeout_seconds=0; expected at least 1",
            ),
            (
                "total_token_order",
                |execution| execution.max_total_tokens = 1,
                "scenario 'persistent_state': execution.max_total_tokens=1 is lower than execution.max_output_tokens=8192; expected max_total_tokens >= max_output_tokens",
            ),
        ];

        for (field, mutate, expected) in cases {
            let mut spec = ScenarioId::PersistentState.spec("run");
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
        let mut spec = ScenarioId::PersistentState.spec("run");
        spec.criteria = vec![CriterionSpec::advisory_judge(
            "durable_result",
            0,
            "invalid",
        )];

        assert_eq!(
            spec.validate().unwrap_err().to_string(),
            "scenario 'persistent_state': criterion 'durable_result' has weight=0; expected at least 1"
        );
    }

    #[test]
    fn validation_reports_duplicate_criterion_indexes() {
        let mut spec = ScenarioId::PersistentState.spec("run");
        spec.criteria = vec![
            CriterionSpec::advisory_judge("duplicate", 50, "first"),
            CriterionSpec::advisory_judge("duplicate", 50, "second"),
        ];

        assert_eq!(
            spec.validate().unwrap_err().to_string(),
            "scenario 'persistent_state': criterion id 'duplicate' is duplicated at indexes 0 and 1; criterion ids must be unique"
        );
    }
}
