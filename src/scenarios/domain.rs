use anyhow::{bail, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ExecutionPolicy;
use crate::artifact::sha256_value;

pub fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

pub fn stable_seed(id: &str) -> u64 {
    id.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

pub fn scenario_contract_sha256(
    case: &ScenarioCase,
    execution_policy: ExecutionPolicy,
) -> Result<String> {
    sha256_value(&serde_json::json!({
        "scenario_id": case.scenario_id,
        "case": case,
        "execution_policy": execution_policy,
    }))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplexityProfile {
    pub planning_depth: u8,
    pub dependency_depth: u8,
    pub parallel_branches: u8,
    pub external_systems: u8,
    pub state_transitions: u16,
    pub wake_cycles: u8,
    pub validation_loops: u8,
    pub artifact_count: u8,
    pub coordination_edges: u16,
    pub ambiguity_level: u8,
    #[serde(default)]
    pub agent_owned_decomposition: bool,
    #[serde(default)]
    pub material_invalidation_events: u8,
    #[serde(default)]
    pub replan_loops: u8,
    #[serde(default)]
    pub compensable_mutations: u8,
    #[serde(default)]
    pub durable_resume_cycles: u8,
    #[serde(default)]
    pub coherent_long_horizon: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComplexityTier {
    L0Atomic,
    L1Sequential,
    L2Stateful,
    L3Concurrent,
    L4Coordinated,
    L5Adaptive,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComplexityMethod {
    #[default]
    Capability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplexityClassification {
    pub method: ComplexityMethod,
    pub tier: ComplexityTier,
    pub profile: ComplexityProfile,
}

impl ComplexityClassification {
    pub fn derive(profile: ComplexityProfile) -> Self {
        Self {
            method: ComplexityMethod::Capability,
            tier: capability_tier(profile),
            profile,
        }
    }
}

fn capability_tier(profile: ComplexityProfile) -> ComplexityTier {
    let stretch_signals = [
        profile.external_systems >= 2,
        profile.parallel_branches >= 2,
        profile.compensable_mutations >= 1,
        profile.durable_resume_cycles >= 1,
        profile.coherent_long_horizon,
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if profile.agent_owned_decomposition
        && profile.material_invalidation_events >= 1
        && profile.replan_loops >= 1
        && stretch_signals >= 2
    {
        ComplexityTier::L5Adaptive
    } else {
        lower_tier(profile)
    }
}

fn lower_tier(profile: ComplexityProfile) -> ComplexityTier {
    if profile.coordination_edges >= 3
        || (profile.dependency_depth >= 3
            && (profile.parallel_branches >= 2 || profile.validation_loops > 0))
    {
        ComplexityTier::L4Coordinated
    } else if profile.parallel_branches >= 2 {
        ComplexityTier::L3Concurrent
    } else if profile.external_systems > 0
        || profile.state_transitions > 0
        || profile.wake_cycles > 0
        || profile.validation_loops > 0
    {
        ComplexityTier::L2Stateful
    } else if profile.planning_depth > 1
        || profile.dependency_depth > 0
        || profile.artifact_count > 0
    {
        ComplexityTier::L1Sequential
    } else {
        ComplexityTier::L0Atomic
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HumanHorizonBasis {
    #[default]
    Unknown,
    AuthorEstimate,
    Measured,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HumanHorizon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_minutes: Option<u32>,
    #[serde(default)]
    pub basis: HumanHorizonBasis,
}

impl HumanHorizon {
    pub fn author_estimate(min_minutes: u32, max_minutes: u32) -> Result<Self> {
        Self::bounded(min_minutes, max_minutes, HumanHorizonBasis::AuthorEstimate)
    }

    pub fn measured(min_minutes: u32, max_minutes: u32) -> Result<Self> {
        Self::bounded(min_minutes, max_minutes, HumanHorizonBasis::Measured)
    }

    fn bounded(min_minutes: u32, max_minutes: u32, basis: HumanHorizonBasis) -> Result<Self> {
        let horizon = Self {
            min_minutes: Some(min_minutes),
            max_minutes: Some(max_minutes),
            basis,
        };
        horizon.validate()?;
        Ok(horizon)
    }

    fn validate(self) -> Result<()> {
        match (self.min_minutes, self.max_minutes, self.basis) {
            (None, None, HumanHorizonBasis::Unknown) => Ok(()),
            (Some(min), Some(max), basis)
                if basis != HumanHorizonBasis::Unknown && min > 0 && min <= max =>
            {
                Ok(())
            }
            _ => bail!("scenario human horizon is inconsistent"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRealism {
    #[default]
    Synthetic,
    RealisticSimulator,
    FrozenRealArtifact,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowMode {
    #[default]
    None,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioRealism {
    #[serde(default)]
    pub execution: ExecutionRealism,
    #[serde(default)]
    pub shadow: ShadowMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioCharacterization {
    #[serde(default)]
    pub human_horizon: HumanHorizon,
    #[serde(default)]
    pub realism: ScenarioRealism,
}

impl ScenarioCharacterization {
    pub fn new(
        human_horizon: HumanHorizon,
        execution: ExecutionRealism,
        shadow: ShadowMode,
    ) -> Result<Self> {
        let characterization = Self {
            human_horizon,
            realism: ScenarioRealism { execution, shadow },
        };
        characterization.validate()?;
        Ok(characterization)
    }

    fn for_scenario(scenario_id: &str) -> Self {
        let execution = match scenario_id {
            "git_regression_forensics" => ExecutionRealism::FrozenRealArtifact,
            "research_pipeline"
            | "security_review"
            | "incident_response"
            | "todo_worker_simple"
            | "todo_worker_planned"
            | "engineering_ticket"
            | "trend_blog"
            | "tool_contract_recovery"
            | "policy_bound_action"
            | "cross_app_transaction"
            | "performance_regression"
            | "browser_cross_site"
            | "swe_config_isolation"
            | "swe_cache_invalidation"
            | "swe_batch_replay"
            | "swe_replay_recovery"
            | "swe_contract_migration"
            | "swe_tenant_isolation"
            | "swe_replay_performance"
            | "swe_release_handoff"
            | "swe_service_journey" => ExecutionRealism::RealisticSimulator,
            _ => ExecutionRealism::Synthetic,
        };
        Self {
            human_horizon: HumanHorizon::default(),
            realism: ScenarioRealism {
                execution,
                shadow: ShadowMode::None,
            },
        }
    }

    fn validate(self) -> Result<()> {
        self.human_horizon.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactExpectation {
    pub id: String,
    pub kind: String,
    pub media_type: String,
    pub schema: Value,
    pub max_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvariantSpec {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DeliverableContract {
    pub artifacts: Vec<ArtifactExpectation>,
    pub invariants: Vec<InvariantSpec>,
    pub provenance_required: bool,
    pub capture_before_cleanup: bool,
}

impl DeliverableContract {
    pub fn validate(&self, scenario_id: &str) -> Result<()> {
        if !self.artifacts.is_empty() && !self.capture_before_cleanup {
            bail!("scenario '{scenario_id}': deliverables must be captured before cleanup");
        }
        let mut artifact_ids = std::collections::HashSet::new();
        for artifact in &self.artifacts {
            if artifact.id.trim().is_empty()
                || artifact.kind.trim().is_empty()
                || artifact.media_type.trim().is_empty()
                || artifact.max_size_bytes == 0
            {
                bail!("scenario '{scenario_id}': deliverable artifact contract is invalid");
            }
            if !artifact
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                bail!(
                    "scenario '{scenario_id}': deliverable artifact id '{}' is not path-safe",
                    artifact.id
                );
            }
            if !artifact_ids.insert(artifact.id.as_str()) {
                bail!(
                    "scenario '{scenario_id}': duplicate deliverable artifact '{}'",
                    artifact.id
                );
            }
        }
        let mut invariant_ids = std::collections::HashSet::new();
        for invariant in &self.invariants {
            if invariant.id.trim().is_empty() || invariant.description.trim().is_empty() {
                bail!("scenario '{scenario_id}': deliverable invariant is invalid");
            }
            if !invariant_ids.insert(invariant.id.as_str()) {
                bail!(
                    "scenario '{scenario_id}': duplicate deliverable invariant '{}'",
                    invariant.id
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioCase {
    pub scenario_id: String,
    /// Digest of the scenario definition this case was materialized from:
    /// prompt, execution policy, criteria, and the case-independent contract.
    /// Two cases with the same digest were evaluated by the same definition.
    /// Sealed by `ScenarioId::materialize`; empty only before sealing.
    pub behavior_sha256: String,
    pub case_id: String,
    pub seed: u64,
    pub inputs: Value,
    pub inputs_sha256: String,
    pub complexity: ComplexityClassification,
    #[serde(default)]
    pub characterization: ScenarioCharacterization,
    pub work: WorkExpectation,
    pub required_capabilities: Vec<String>,
    pub deliverable_contract: DeliverableContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkExpectation {
    pub minimum_expected_work: u64,
}

impl ScenarioCase {
    pub fn new(
        scenario_id: impl Into<String>,
        seed: u64,
        inputs: Value,
        profile: ComplexityProfile,
        required_capabilities: Vec<String>,
        deliverable_contract: DeliverableContract,
    ) -> Result<Self> {
        let scenario_id = scenario_id.into();
        let work = WorkExpectation {
            minimum_expected_work: minimum_expected_work(profile),
        };
        let characterization = ScenarioCharacterization::for_scenario(&scenario_id);
        let case = Self {
            case_id: format!("{scenario_id}:seed-{seed:016x}"),
            scenario_id,
            behavior_sha256: String::new(),
            seed,
            inputs_sha256: sha256_value(&inputs)?,
            inputs,
            complexity: ComplexityClassification::derive(profile),
            characterization,
            work,
            required_capabilities,
            deliverable_contract,
        };
        case.validate_shape()?;
        Ok(case)
    }

    /// Bind the case to the definition it was materialized from. The digest
    /// covers everything the evaluation depends on except the seed-specific
    /// inputs, so it identifies the definition rather than the case.
    pub fn seal(mut self, behavior_sha256: String) -> Result<Self> {
        self.behavior_sha256 = behavior_sha256;
        self.validate()?;
        Ok(self)
    }

    pub fn with_minimum_expected_work(mut self, minimum_expected_work: u64) -> Result<Self> {
        self.work.minimum_expected_work = minimum_expected_work;
        self.behavior_sha256.clear();
        self.validate_shape()?;
        Ok(self)
    }

    pub fn with_characterization(
        mut self,
        characterization: ScenarioCharacterization,
    ) -> Result<Self> {
        self.characterization = characterization;
        self.behavior_sha256.clear();
        self.validate_shape()?;
        Ok(self)
    }

    /// Seal with a fixed digest for tests that build cases by hand.
    #[cfg(test)]
    pub(crate) fn sealed_for_tests(self) -> Self {
        self.seal(crate::artifact::sha256_bytes(b"test definition"))
            .expect("hand-built test case seals")
    }

    /// A sealed case: its shape is valid and it names its definition digest.
    pub fn validate(&self) -> Result<()> {
        self.validate_shape()?;
        if !is_sha256(&self.behavior_sha256) {
            bail!(
                "scenario case '{}' has no valid behavior_sha256",
                self.case_id
            );
        }
        Ok(())
    }

    pub(super) fn validate_shape(&self) -> Result<()> {
        if self.scenario_id.trim().is_empty() {
            bail!("scenario case identity is invalid");
        }
        if self.case_id.trim().is_empty() {
            bail!("scenario case id is empty");
        }
        let expected_case_id = format!("{}:seed-{:016x}", self.scenario_id, self.seed);
        if self.case_id != expected_case_id {
            bail!("scenario case id is inconsistent with its materialized identity");
        }
        if sha256_value(&self.inputs)? != self.inputs_sha256 {
            bail!("scenario case inputs do not match inputs_sha256");
        }
        if self.complexity != ComplexityClassification::derive(self.complexity.profile) {
            bail!("scenario case complexity classification is inconsistent");
        }
        self.characterization.validate()?;
        if self.work.minimum_expected_work == 0 {
            bail!("scenario case minimum work expectation is invalid");
        }
        if self
            .required_capabilities
            .iter()
            .any(|capability| capability.trim().is_empty())
        {
            bail!("scenario case has an empty required capability");
        }
        let unique_capabilities = self
            .required_capabilities
            .iter()
            .collect::<std::collections::HashSet<_>>();
        if unique_capabilities.len() != self.required_capabilities.len() {
            bail!("scenario case has duplicate required capabilities");
        }
        self.deliverable_contract.validate(&self.scenario_id)
    }
}

fn minimum_expected_work(profile: ComplexityProfile) -> u64 {
    1_u64
        .saturating_add(u64::from(profile.planning_depth))
        .saturating_add(u64::from(profile.artifact_count))
        .saturating_add(u64::from(profile.coordination_edges))
        .saturating_add(u64::from(profile.validation_loops))
        .saturating_add(u64::from(profile.wake_cycles))
}

#[derive(Debug, Clone)]
pub struct CapturedDeliverable {
    pub id: String,
    pub kind: String,
    pub content: CapturedDeliverableContent,
    pub invariants: Vec<CapturedInvariant>,
    pub provenance: Vec<ProvenanceEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", content = "content", rename_all = "snake_case")]
pub enum CapturedDeliverableContent {
    Json(Value),
    TextUtf8(String),
}

impl Default for CapturedDeliverableContent {
    fn default() -> Self {
        Self::Json(Value::Null)
    }
}

impl CapturedDeliverableContent {
    pub fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::TextUtf8(_) => None,
        }
    }
}

impl From<Value> for CapturedDeliverableContent {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

impl From<String> for CapturedDeliverableContent {
    fn from(value: String) -> Self {
        Self::TextUtf8(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CapturedInvariant {
    pub id: String,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProvenanceEvidence {
    pub kind: String,
    pub source_id: String,
    pub relation: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complexity_tiers_are_derived_from_the_vector() {
        assert_eq!(
            ComplexityClassification::derive(ComplexityProfile::default()).tier,
            ComplexityTier::L0Atomic
        );
        assert_eq!(
            ComplexityClassification::derive(ComplexityProfile {
                external_systems: 1,
                state_transitions: 2,
                ..ComplexityProfile::default()
            })
            .tier,
            ComplexityTier::L2Stateful
        );
        assert_eq!(
            ComplexityClassification::derive(ComplexityProfile {
                dependency_depth: 3,
                validation_loops: 1,
                coordination_edges: 4,
                ..ComplexityProfile::default()
            })
            .tier,
            ComplexityTier::L4Coordinated
        );
        let formerly_adaptive = ComplexityProfile {
            ambiguity_level: 8,
            validation_loops: 2,
            ..ComplexityProfile::default()
        };
        assert_eq!(
            ComplexityClassification::derive(formerly_adaptive).tier,
            ComplexityTier::L2Stateful
        );
        assert_eq!(
            ComplexityClassification::derive(ComplexityProfile {
                agent_owned_decomposition: true,
                material_invalidation_events: 1,
                replan_loops: 1,
                external_systems: 2,
                compensable_mutations: 1,
                ..ComplexityProfile::default()
            })
            .tier,
            ComplexityTier::L5Adaptive
        );
    }

    #[test]
    fn case_hash_detects_materialized_input_changes() {
        let mut case = ScenarioCase::new(
            "case",
            7,
            serde_json::json!({"value": 1}),
            ComplexityProfile::default(),
            vec![],
            DeliverableContract::default(),
        )
        .unwrap();
        case.inputs = serde_json::json!({"value": 2});
        assert!(case.validate().is_err());
    }

    #[test]
    fn characterization_builders_validate_horizon_and_shadow() {
        assert!(HumanHorizon::author_estimate(0, 60).is_err());
        assert!(HumanHorizon::measured(120, 60).is_err());
        let characterization = ScenarioCharacterization::new(
            HumanHorizon::author_estimate(60, 120).unwrap(),
            ExecutionRealism::RealisticSimulator,
            ShadowMode::ReadOnly,
        )
        .unwrap();
        let case = ScenarioCase::new(
            "future_l5",
            7,
            serde_json::json!({}),
            ComplexityProfile::default(),
            vec![],
            DeliverableContract::default(),
        )
        .unwrap()
        .with_characterization(characterization)
        .unwrap();
        assert_eq!(case.characterization, characterization);
    }
}
