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

/// What a scenario needs from the engine, the runner host or a fixture to run.
/// The closed list keeps the catalog honest: a new need is a new variant, and
/// every id is spelled once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Capability {
    #[serde(rename = "browser::interactive")]
    BrowserInteractive,
    #[serde(rename = "cross_repo_contract_simulator::v1")]
    CrossRepoContractSimulatorV1,
    #[serde(rename = "curl")]
    Curl,
    #[serde(rename = "docker")]
    Docker,
    #[serde(rename = "e2e::adaptive-flow-v1")]
    E2eAdaptiveFlowV1,
    #[serde(rename = "e2e::control-plane-v1")]
    E2eControlPlaneV1,
    #[serde(rename = "e2e::filesystem")]
    E2eFilesystem,
    #[serde(rename = "e2e::git")]
    E2eGit,
    #[serde(rename = "e2e::run-scoped-fixtures")]
    E2eRunScopedFixtures,
    #[serde(rename = "e2e::shell")]
    E2eShell,
    #[serde(rename = "e2e::subagents")]
    E2eSubagents,
    #[serde(rename = "e2e::workflow-resume-v1")]
    E2eWorkflowResumeV1,
    #[serde(rename = "fixture::multi-origin-http")]
    FixtureMultiOriginHttp,
    #[serde(rename = "git")]
    Git,
    #[serde(rename = "git::deterministic-fixture-v1")]
    GitDeterministicFixtureV1,
    #[serde(rename = "git::offline-bundle")]
    GitOfflineBundle,
    #[serde(rename = "github::security-read")]
    GithubSecurityRead,
    #[serde(rename = "github::trusted-handoff")]
    GithubTrustedHandoff,
    #[serde(rename = "harness::independent_session")]
    HarnessIndependentSession,
    #[serde(rename = "harness::post-turn-validation")]
    HarnessPostTurnValidation,
    #[serde(rename = "harness::scripted-dialogue-v1")]
    HarnessScriptedDialogueV1,
    #[serde(rename = "iii::coder")]
    IiiCoder,
    #[serde(rename = "iii::compose")]
    IiiCompose,
    #[serde(rename = "iii::database")]
    IiiDatabase,
    #[serde(rename = "iii::functions")]
    IiiFunctions,
    #[serde(rename = "iii::registry")]
    IiiRegistry,
    #[serde(rename = "iii::shell")]
    IiiShell,
    #[serde(rename = "iii::state")]
    IiiState,
    #[serde(rename = "iii::triggers")]
    IiiTriggers,
    #[serde(rename = "iii::workers")]
    IiiWorkers,
    #[serde(rename = "incident_fixture::v1")]
    IncidentFixtureV1,
    #[serde(rename = "node")]
    Node,
    #[serde(rename = "playwright")]
    Playwright,
    #[serde(rename = "python3")]
    Python3,
    #[serde(rename = "release_shadow::read-only-v1")]
    ReleaseShadowReadOnlyV1,
    #[serde(rename = "release_train_simulator::v1")]
    ReleaseTrainSimulatorV1,
    #[serde(rename = "security_scan::on-demand")]
    SecurityScanOnDemand,
    #[serde(rename = "security_scan::v1")]
    SecurityScanV1,
    #[serde(rename = "swe::isolated-python-workspace")]
    SweIsolatedPythonWorkspace,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BrowserInteractive => "browser::interactive",
            Self::CrossRepoContractSimulatorV1 => "cross_repo_contract_simulator::v1",
            Self::Curl => "curl",
            Self::Docker => "docker",
            Self::E2eAdaptiveFlowV1 => "e2e::adaptive-flow-v1",
            Self::E2eControlPlaneV1 => "e2e::control-plane-v1",
            Self::E2eFilesystem => "e2e::filesystem",
            Self::E2eGit => "e2e::git",
            Self::E2eRunScopedFixtures => "e2e::run-scoped-fixtures",
            Self::E2eShell => "e2e::shell",
            Self::E2eSubagents => "e2e::subagents",
            Self::E2eWorkflowResumeV1 => "e2e::workflow-resume-v1",
            Self::FixtureMultiOriginHttp => "fixture::multi-origin-http",
            Self::Git => "git",
            Self::GitDeterministicFixtureV1 => "git::deterministic-fixture-v1",
            Self::GitOfflineBundle => "git::offline-bundle",
            Self::GithubSecurityRead => "github::security-read",
            Self::GithubTrustedHandoff => "github::trusted-handoff",
            Self::HarnessIndependentSession => "harness::independent_session",
            Self::HarnessPostTurnValidation => "harness::post-turn-validation",
            Self::HarnessScriptedDialogueV1 => "harness::scripted-dialogue-v1",
            Self::IiiCoder => "iii::coder",
            Self::IiiCompose => "iii::compose",
            Self::IiiDatabase => "iii::database",
            Self::IiiFunctions => "iii::functions",
            Self::IiiRegistry => "iii::registry",
            Self::IiiShell => "iii::shell",
            Self::IiiState => "iii::state",
            Self::IiiTriggers => "iii::triggers",
            Self::IiiWorkers => "iii::workers",
            Self::IncidentFixtureV1 => "incident_fixture::v1",
            Self::Node => "node",
            Self::Playwright => "playwright",
            Self::Python3 => "python3",
            Self::ReleaseShadowReadOnlyV1 => "release_shadow::read-only-v1",
            Self::ReleaseTrainSimulatorV1 => "release_train_simulator::v1",
            Self::SecurityScanOnDemand => "security_scan::on-demand",
            Self::SecurityScanV1 => "security_scan::v1",
            Self::SweIsolatedPythonWorkspace => "swe::isolated-python-workspace",
        }
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

    /// A synthetic exercise of the engine, with no real artifact behind it.
    pub fn synthetic() -> Self {
        Self::with_execution(ExecutionRealism::Synthetic)
    }

    /// A realistic simulator of the product surface under test.
    pub fn realistic() -> Self {
        Self::with_execution(ExecutionRealism::RealisticSimulator)
    }

    /// A frozen real artifact, such as a real repository at a pinned revision.
    pub fn frozen_real_artifact() -> Self {
        Self::with_execution(ExecutionRealism::FrozenRealArtifact)
    }

    fn with_execution(execution: ExecutionRealism) -> Self {
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
    #[serde(default)]
    pub characterization: ScenarioCharacterization,
    pub required_capabilities: Vec<Capability>,
    pub deliverable_contract: DeliverableContract,
}

impl ScenarioCase {
    pub fn new(
        scenario_id: impl Into<String>,
        seed: u64,
        inputs: Value,
        required_capabilities: Vec<Capability>,
        deliverable_contract: DeliverableContract,
    ) -> Result<Self> {
        let scenario_id = scenario_id.into();
        let characterization = ScenarioCharacterization::synthetic();
        let case = Self {
            case_id: format!("{scenario_id}:seed-{seed:016x}"),
            scenario_id,
            behavior_sha256: String::new(),
            seed,
            inputs_sha256: sha256_value(&inputs)?,
            inputs,
            characterization,
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
        self.characterization.validate()?;
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
    fn case_hash_detects_materialized_input_changes() {
        let mut case = ScenarioCase::new(
            "case",
            7,
            serde_json::json!({"value": 1}),
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
            vec![],
            DeliverableContract::default(),
        )
        .unwrap()
        .with_characterization(characterization)
        .unwrap();
        assert_eq!(case.characterization, characterization);
    }
}
