use anyhow::{bail, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::sha256_value;

pub const COMPLEXITY_POLICY_VERSION: u32 = 1;
pub const WORK_EXPECTATION_POLICY_VERSION: u32 = 1;

pub fn stable_seed(id: &str) -> u64 {
    id.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplexityClassification {
    pub policy_version: u32,
    pub tier: ComplexityTier,
    pub profile: ComplexityProfile,
}

impl ComplexityClassification {
    pub fn derive(profile: ComplexityProfile) -> Self {
        let tier = if profile.ambiguity_level >= 7
            && (profile.validation_loops >= 2 || profile.wake_cycles >= 2)
        {
            ComplexityTier::L5Adaptive
        } else if profile.coordination_edges >= 3
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
        };
        Self {
            policy_version: COMPLEXITY_POLICY_VERSION,
            tier,
            profile,
        }
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
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub inputs: Value,
    pub inputs_sha256: String,
    pub complexity: ComplexityClassification,
    pub work: WorkExpectation,
    pub required_capabilities: Vec<String>,
    pub deliverable_contract: DeliverableContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkExpectation {
    pub policy_version: u32,
    pub minimum_expected_work: u64,
}

impl ScenarioCase {
    pub fn new(
        scenario_id: impl Into<String>,
        scenario_version: u32,
        seed: u64,
        inputs: Value,
        profile: ComplexityProfile,
        required_capabilities: Vec<String>,
        deliverable_contract: DeliverableContract,
    ) -> Result<Self> {
        let scenario_id = scenario_id.into();
        let work = WorkExpectation {
            policy_version: WORK_EXPECTATION_POLICY_VERSION,
            minimum_expected_work: minimum_expected_work(profile),
        };
        let case = Self {
            case_id: format!("{scenario_id}:v{scenario_version}:seed-{seed:016x}"),
            scenario_id,
            scenario_version,
            seed,
            inputs_sha256: sha256_value(&inputs)?,
            inputs,
            complexity: ComplexityClassification::derive(profile),
            work,
            required_capabilities,
            deliverable_contract,
        };
        case.validate()?;
        Ok(case)
    }

    pub fn with_minimum_expected_work(mut self, minimum_expected_work: u64) -> Result<Self> {
        self.work.minimum_expected_work = minimum_expected_work;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<()> {
        if self.scenario_id.trim().is_empty() || self.scenario_version == 0 {
            bail!("scenario case identity is invalid");
        }
        if self.case_id.trim().is_empty() {
            bail!("scenario case id is empty");
        }
        let expected_case_id = format!(
            "{}:v{}:seed-{:016x}",
            self.scenario_id, self.scenario_version, self.seed
        );
        if self.case_id != expected_case_id {
            bail!("scenario case id is inconsistent with its materialized identity");
        }
        if sha256_value(&self.inputs)? != self.inputs_sha256 {
            bail!("scenario case inputs do not match inputs_sha256");
        }
        if self.complexity.policy_version != COMPLEXITY_POLICY_VERSION
            || self.complexity != ComplexityClassification::derive(self.complexity.profile)
        {
            bail!("scenario case complexity classification is inconsistent");
        }
        if self.work.policy_version != WORK_EXPECTATION_POLICY_VERSION
            || self.work.minimum_expected_work == 0
        {
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
    pub content: Value,
    pub invariants: Vec<CapturedInvariant>,
    pub provenance: Vec<ProvenanceEvidence>,
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
    }

    #[test]
    fn case_hash_detects_materialized_input_changes() {
        let mut case = ScenarioCase::new(
            "case",
            1,
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
}
