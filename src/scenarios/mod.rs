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
pub mod common;
pub mod custom_validator;
pub mod direct_answer;
mod domain;
pub mod engineering_ticket;
pub mod git_regression_forensics;
pub mod incident_response;
pub mod mechanical_reaction;
pub mod multi_subagent_validation;
pub mod persistent_state;
pub mod reactive_automation;
pub mod receiving_operation;
pub mod research_pipeline;
pub mod security_review;
pub mod shell_coder_sandbox;
pub mod subagent_validation;
pub mod subagent_validation_failure;
pub mod timer_wake;
pub mod todo_worker;
pub mod validation_chain;
pub mod validation_loop;
pub mod validation_scope_enforcement;
pub mod validation_self_repair;

pub use domain::{
    scenario_contract_sha256, stable_seed, ArtifactExpectation, CapturedDeliverable,
    CapturedDeliverableContent, CapturedInvariant, ComplexityProfile, ComplexityTier,
    DeliverableContract, InvariantSpec, ProvenanceEvidence, ScenarioCase, WorkExpectation,
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
    #[value(name = "incident_response")]
    IncidentResponse,
    #[value(name = "todo_worker_simple")]
    TodoWorkerSimple,
    #[value(name = "todo_worker_planned")]
    TodoWorkerPlanned,
    #[value(name = "todo_worker_self_validating")]
    TodoWorkerSelfValidating,
    #[value(name = "engineering_ticket")]
    EngineeringTicket,
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
}

impl ScenarioId {
    pub const ALL: [Self; 23] = [
        Self::DirectAnswer,
        Self::PersistentState,
        Self::ReactiveAutomation,
        Self::ShellCoderSandbox,
        Self::ResearchPipeline,
        Self::SecurityReview,
        Self::IncidentResponse,
        Self::TodoWorkerSimple,
        Self::TodoWorkerPlanned,
        Self::TodoWorkerSelfValidating,
        Self::EngineeringTicket,
        Self::GitRegressionForensics,
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
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectAnswer => direct_answer::ID,
            Self::PersistentState => persistent_state::ID,
            Self::ReactiveAutomation => reactive_automation::ID,
            Self::ShellCoderSandbox => shell_coder_sandbox::ID,
            Self::ResearchPipeline => research_pipeline::ID,
            Self::SecurityReview => security_review::ID,
            Self::IncidentResponse => incident_response::ID,
            Self::TodoWorkerSimple => todo_worker::SIMPLE_ID,
            Self::TodoWorkerPlanned => todo_worker::PLANNED_ID,
            Self::TodoWorkerSelfValidating => todo_worker::SELF_VALIDATING_ID,
            Self::EngineeringTicket => engineering_ticket::ID,
            Self::GitRegressionForensics => git_regression_forensics::ID,
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
            Self::IncidentResponse => incident_response::scenario(run_id),
            Self::TodoWorkerSimple => todo_worker::simple_scenario(run_id),
            Self::TodoWorkerPlanned => todo_worker::planned_scenario(run_id),
            Self::TodoWorkerSelfValidating => todo_worker::self_validating_scenario(run_id),
            Self::EngineeringTicket => engineering_ticket::scenario(run_id),
            Self::GitRegressionForensics => git_regression_forensics::scenario(run_id),
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
            Self::IncidentResponse => incident_response::materialize(namespace, seed)?,
            Self::TodoWorkerSimple => todo_worker::simple_materialize(namespace, seed)?,
            Self::TodoWorkerPlanned => todo_worker::planned_materialize(namespace, seed)?,
            Self::TodoWorkerSelfValidating => {
                todo_worker::self_validating_materialize(namespace, seed)?
            }
            Self::EngineeringTicket => engineering_ticket::materialize(namespace, seed)?,
            Self::GitRegressionForensics => git_regression_forensics::materialize(namespace, seed)?,
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
        };
        materialized.validate()?;
        Ok(materialized)
    }

    pub fn canonical_seed(self) -> u64 {
        if self == Self::EngineeringTicket {
            return engineering_ticket::CANONICAL_SEED;
        }
        // Stable FNV-1a keeps canonical cases reproducible without tying their
        // identity to a particular execution or retry attempt.
        stable_seed(self.as_str())
    }

    pub fn execution_kind(self) -> ScenarioExecutionKind {
        match self {
            Self::SecurityReview | Self::IncidentResponse | Self::TodoWorkerPlanned => {
                ScenarioExecutionKind::CompositeFlow
            }
            _ => ScenarioExecutionKind::HarnessTurn,
        }
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
    fn registry_contains_twenty_three_unique_valid_scenarios() {
        let mut ids = HashSet::new();
        for scenario in ScenarioId::ALL {
            assert!(ids.insert(scenario.as_str()));
            scenario.spec("run").validate().unwrap();
            scenario
                .materialize("run", scenario.canonical_seed())
                .unwrap();
        }
        assert_eq!(ids.len(), 23);
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
    fn default_selection_includes_every_registered_scenario() {
        let selected = selected(&[]);
        assert!(selected.contains(&ScenarioId::SecurityReview));
        assert!(selected.contains(&ScenarioId::IncidentResponse));
        assert!(selected.contains(&ScenarioId::EngineeringTicket));
        assert!(selected.contains(&ScenarioId::GitRegressionForensics));
        assert!(selected.contains(&ScenarioId::TodoWorkerSimple));
        assert!(selected.contains(&ScenarioId::TodoWorkerPlanned));
        assert!(selected.contains(&ScenarioId::TodoWorkerSelfValidating));
        assert_eq!(selected.len(), ScenarioId::ALL.len());
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
                |execution| execution.max_total_tokens = Some(0),
                "scenario 'persistent_state': execution.max_total_tokens=0; expected None (unbounded) or at least 1",
            ),
            (
                "stuck_timeout_seconds",
                |execution| execution.stuck_timeout_seconds = 0,
                "scenario 'persistent_state': execution.stuck_timeout_seconds=0; expected at least 1",
            ),
            (
                "total_token_order",
                |execution| execution.max_total_tokens = Some(1),
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
