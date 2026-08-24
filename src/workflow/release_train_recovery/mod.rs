use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod adaptive_runtime;

pub use adaptive_runtime::{
    adaptive_policy, build_adaptive_runtime, reference_adaptive_plans, ReleaseTrainAdaptiveRuntime,
    ReleaseTrainRuntimeState,
};

pub const SCENARIO_ID: &str = "release_train_recovery";
pub const SCENARIO_VERSION: u32 = 1;
pub const INVALIDATION_EVIDENCE_ID: &str = "promotion_preview.incompatible_latest_graph";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TemplatePhase {
    TrustedAnchor,
    AgentSelected,
    DeterministicGate,
    ProductMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplaySafety {
    Idempotent,
    Compensable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateDescriptor {
    pub id: String,
    pub revision: u8,
    pub phase: TemplatePhase,
    pub replay_safety: ReplaySafety,
    pub mutates_product: bool,
    pub network_write_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanRevisionRequest {
    pub revision: u8,
    #[serde(default)]
    pub selected_templates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_sha256: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveNode {
    pub id: String,
    pub template_id: String,
    pub depends_on: Vec<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterializedAdaptiveDag {
    pub scenario_id: String,
    pub scenario_version: u32,
    pub revision: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_sha256: Option<String>,
    pub evidence_ids: Vec<String>,
    pub nodes: Vec<AdaptiveNode>,
    pub sha256: String,
}

pub fn template_catalog() -> Vec<TemplateDescriptor> {
    [
        (
            "preflight_release_identity",
            1,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "inspect_partial_publication",
            1,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "inspect_asset_manifest",
            1,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "inspect_registry_pointer",
            1,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "rerun_same_immutable_run",
            1,
            TemplatePhase::ProductMutation,
            ReplaySafety::Idempotent,
            true,
        ),
        (
            "verify_exact_publication",
            1,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "preview_promotion",
            1,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "resume_from_invalidation",
            2,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "inspect_incompatible_graph",
            2,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "inspect_operation_history",
            2,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "reject_stale_null_cas",
            2,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "create_fresh_gated_operation",
            2,
            TemplatePhase::ProductMutation,
            ReplaySafety::Compensable,
            true,
        ),
        (
            "observe_stale_canary",
            2,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "verify_release_convergence",
            2,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
            false,
        ),
        (
            "reconcile_release_state",
            2,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Idempotent,
            false,
        ),
    ]
    .into_iter()
    .map(
        |(id, revision, phase, replay_safety, mutates_product)| TemplateDescriptor {
            id: id.into(),
            revision,
            phase,
            replay_safety,
            mutates_product,
            network_write_allowed: false,
        },
    )
    .collect()
}

pub fn materialize_plan(request: &PlanRevisionRequest) -> Result<MaterializedAdaptiveDag> {
    if !matches!(request.revision, 1 | 2) {
        bail!("release train recovery supports exactly two plan revisions");
    }
    if request.revision == 1 {
        if request.supersedes_sha256.is_some() || !request.evidence_ids.is_empty() {
            bail!("revision 1 cannot supersede a plan or cite invalidation evidence");
        }
    } else {
        let supersedes = request
            .supersedes_sha256
            .as_deref()
            .filter(|value| is_sha256(value))
            .context("revision 2 requires a valid supersedes_sha256")?;
        if !request
            .evidence_ids
            .iter()
            .any(|value| value == INVALIDATION_EVIDENCE_ID)
        {
            bail!("revision 2 must cite '{INVALIDATION_EVIDENCE_ID}'");
        }
        if supersedes.chars().all(|value| value == '0') {
            bail!("revision 2 cannot supersede an all-zero hash");
        }
    }

    let catalog = template_catalog()
        .into_iter()
        .map(|descriptor| (descriptor.id.clone(), descriptor))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();
    for template_id in &request.selected_templates {
        let descriptor = catalog
            .get(template_id)
            .with_context(|| format!("unknown release recovery template '{template_id}'"))?;
        if descriptor.revision != request.revision
            || descriptor.phase != TemplatePhase::AgentSelected
        {
            bail!(
                "template '{template_id}' is not agent-selectable in revision {}",
                request.revision
            );
        }
        if !selected.insert(template_id.clone()) {
            bail!("template '{template_id}' was selected more than once");
        }
    }
    if selected.is_empty() || selected.len() > 3 {
        bail!(
            "each release recovery revision must select between one and three analysis templates"
        );
    }

    let nodes = if request.revision == 1 {
        first_revision_nodes(selected.into_iter().collect())
    } else {
        second_revision_nodes(selected.into_iter().collect())
    };
    let unsigned = (
        SCENARIO_ID,
        SCENARIO_VERSION,
        request.revision,
        &request.supersedes_sha256,
        &request.evidence_ids,
        &nodes,
    );
    let sha256 = canonical_sha256(&unsigned)?;
    Ok(MaterializedAdaptiveDag {
        scenario_id: SCENARIO_ID.into(),
        scenario_version: SCENARIO_VERSION,
        revision: request.revision,
        supersedes_sha256: request.supersedes_sha256.clone(),
        evidence_ids: request.evidence_ids.clone(),
        nodes,
        sha256,
    })
}

fn first_revision_nodes(selected: Vec<String>) -> Vec<AdaptiveNode> {
    let mut nodes = vec![node("preflight", "preflight_release_identity", &[])];
    let mut analyses = Vec::new();
    for (index, template) in selected.into_iter().enumerate() {
        let id = format!("analysis_{}", index + 1);
        analyses.push(id.clone());
        nodes.push(AdaptiveNode {
            id,
            template_id: template,
            depends_on: vec!["preflight".into()],
            required: true,
        });
    }
    nodes.push(AdaptiveNode {
        id: "rerun_same_run".into(),
        template_id: "rerun_same_immutable_run".into(),
        depends_on: analyses,
        required: true,
    });
    nodes.push(node(
        "verify_publication",
        "verify_exact_publication",
        &["rerun_same_run"],
    ));
    nodes.push(node(
        "preview",
        "preview_promotion",
        &["verify_publication"],
    ));
    nodes
}

fn second_revision_nodes(selected: Vec<String>) -> Vec<AdaptiveNode> {
    let mut nodes = vec![node("resume", "resume_from_invalidation", &[])];
    let mut analyses = Vec::new();
    for (index, template) in selected.into_iter().enumerate() {
        let id = format!("replan_analysis_{}", index + 1);
        analyses.push(id.clone());
        nodes.push(AdaptiveNode {
            id,
            template_id: template,
            depends_on: vec!["resume".into()],
            required: true,
        });
    }
    nodes.push(AdaptiveNode {
        id: "reject_stale".into(),
        template_id: "reject_stale_null_cas".into(),
        depends_on: analyses,
        required: true,
    });
    nodes.push(node(
        "create_fresh",
        "create_fresh_gated_operation",
        &["reject_stale"],
    ));
    nodes.push(node(
        "stale_canary",
        "observe_stale_canary",
        &["create_fresh"],
    ));
    nodes.push(node(
        "converge",
        "verify_release_convergence",
        &["stale_canary"],
    ));
    nodes.push(node("reconcile", "reconcile_release_state", &["converge"]));
    nodes
}

fn node(id: &str, template_id: &str, depends_on: &[&str]) -> AdaptiveNode {
    AdaptiveNode {
        id: id.into(),
        template_id: template_id.into(),
        depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
        required: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Cancelled,
    Succeeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Pending,
    Rejected,
    Running,
    Succeeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImmutableTag {
    pub name: String,
    pub version: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRun {
    pub run_id: u64,
    pub attempt: u32,
    pub status: RunStatus,
    pub required_assets: BTreeSet<String>,
    pub published_assets: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromotionOperation {
    pub id: String,
    pub expected_latest: Option<String>,
    pub target_version: String,
    pub status: OperationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseFixture {
    pub tag: ImmutableTag,
    pub run: ReleaseRun,
    pub exact_version_published: bool,
    pub latest_version: String,
    pub latest_graph_compatible: bool,
    pub stale_operation: PromotionOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTrainState {
    pub tag: ImmutableTag,
    pub original_tag: ImmutableTag,
    pub run: ReleaseRun,
    pub exact_version_published: bool,
    pub latest_version: String,
    pub latest_graph_compatible: bool,
    pub stale_operation: PromotionOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_operation: Option<PromotionOperation>,
    pub previewed: bool,
    pub latest_cas_count: u32,
    pub canary_reads: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseAction {
    RerunSameImmutableRun {
        run_id: u64,
        tag: String,
        version: String,
    },
    PreviewPromotion,
    RejectStaleNullCas {
        operation_id: String,
    },
    RetryStaleOperation {
        operation_id: String,
    },
    CreateFreshGatedOperation {
        expected_latest: String,
    },
    ObserveCanary,
    Retag {
        digest: String,
    },
    BumpVersion {
        version: String,
    },
    DirectLatestMutation {
        version: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimulationEvent {
    pub id: String,
    pub summary: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ReleaseTrainSimulator {
    pub state: ReleaseTrainState,
    audit: Vec<SimulationEvent>,
}

impl ReleaseTrainSimulator {
    pub fn from_fixture_path(path: &Path) -> Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("read release fixture {}", path.display()))?;
        let fixture: ReleaseFixture = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse release fixture {}", path.display()))?;
        Self::new(fixture)
    }

    pub fn new(fixture: ReleaseFixture) -> Result<Self> {
        validate_fixture(&fixture)?;
        Ok(Self {
            state: ReleaseTrainState {
                original_tag: fixture.tag.clone(),
                tag: fixture.tag,
                run: fixture.run,
                exact_version_published: fixture.exact_version_published,
                latest_version: fixture.latest_version,
                latest_graph_compatible: fixture.latest_graph_compatible,
                stale_operation: fixture.stale_operation,
                fresh_operation: None,
                previewed: false,
                latest_cas_count: 0,
                canary_reads: Vec::new(),
            },
            audit: Vec::new(),
        })
    }

    pub fn audit(&self) -> &[SimulationEvent] {
        &self.audit
    }

    pub fn apply(&mut self, action: ReleaseAction) -> Result<SimulationEvent> {
        let event = match action {
            ReleaseAction::RerunSameImmutableRun {
                run_id,
                tag,
                version,
            } => {
                if run_id != self.state.run.run_id
                    || tag != self.state.tag.name
                    || version != self.state.tag.version
                {
                    bail!("recovery must reuse the exact run id, immutable tag, and version");
                }
                if self.state.run.status != RunStatus::Cancelled || self.state.run.attempt != 1 {
                    bail!("only cancelled attempt 1 can be recovered");
                }
                self.state.run.attempt = 2;
                self.state.run.status = RunStatus::Succeeded;
                self.state.run.published_assets = self.state.run.required_assets.clone();
                self.state.exact_version_published = true;
                event(
                    "release.run_recovered",
                    "attempt 2 completed for the same immutable run",
                    &["run.attempt_2", "registry.exact_version"],
                )
            }
            ReleaseAction::PreviewPromotion => {
                require_publication_complete(&self.state)?;
                self.state.previewed = true;
                event(
                    "promotion.previewed",
                    "preview exposed an incompatible historical latest graph",
                    &[INVALIDATION_EVIDENCE_ID],
                )
            }
            ReleaseAction::RejectStaleNullCas { operation_id } => {
                if operation_id != self.state.stale_operation.id
                    || self.state.stale_operation.expected_latest.is_some()
                {
                    bail!("only the known stale null-CAS operation can be rejected");
                }
                self.state.stale_operation.status = OperationStatus::Rejected;
                event(
                    "promotion.stale_rejected",
                    "stale operation with null expected_latest was rejected",
                    &["operation.stale_null_cas"],
                )
            }
            ReleaseAction::RetryStaleOperation { operation_id } => {
                if operation_id == self.state.stale_operation.id
                    && self.state.stale_operation.expected_latest.is_none()
                {
                    bail!("retrying the stale null-CAS operation is forbidden; create a fresh gated operation");
                }
                bail!("operation is not the stale fixture operation");
            }
            ReleaseAction::CreateFreshGatedOperation { expected_latest } => {
                if !self.state.previewed
                    || self.state.stale_operation.status != OperationStatus::Rejected
                {
                    bail!("fresh promotion requires preview evidence and explicit stale-operation rejection");
                }
                if expected_latest != self.state.latest_version {
                    bail!(
                        "fresh promotion expected_latest must preserve the actual latest pointer"
                    );
                }
                if self.state.fresh_operation.is_some() {
                    bail!("exactly one fresh promotion operation is allowed");
                }
                self.state.fresh_operation = Some(PromotionOperation {
                    id: "promotion-fresh-001".into(),
                    expected_latest: Some(expected_latest),
                    target_version: self.state.tag.version.clone(),
                    status: OperationStatus::Pending,
                });
                event(
                    "promotion.fresh_created",
                    "fresh gated promotion preserved the actual latest pointer",
                    &["operation.fresh", "cas.expected_latest"],
                )
            }
            ReleaseAction::ObserveCanary => self.observe_canary()?,
            ReleaseAction::Retag { digest } => {
                bail!("immutable tag cannot be changed to digest '{digest}'")
            }
            ReleaseAction::BumpVersion { version } => {
                bail!("recovery cannot replace the exact version with '{version}'")
            }
            ReleaseAction::DirectLatestMutation { version } => {
                bail!("direct latest mutation to '{version}' bypasses the gated CAS operation")
            }
        };
        self.audit.push(event.clone());
        Ok(event)
    }

    fn observe_canary(&mut self) -> Result<SimulationEvent> {
        let operation = self
            .state
            .fresh_operation
            .as_mut()
            .context("canary observation requires a fresh gated operation")?;
        match operation.status {
            OperationStatus::Pending => {
                let expected = operation
                    .expected_latest
                    .as_ref()
                    .context("fresh operation is missing expected_latest")?;
                if expected != &self.state.latest_version {
                    bail!("latest pointer changed before the fresh operation CAS");
                }
                let stale = self.state.latest_version.clone();
                self.state.latest_version = operation.target_version.clone();
                self.state.latest_cas_count += 1;
                operation.status = OperationStatus::Running;
                self.state.canary_reads.push(stale);
                Ok(event(
                    "canary.stale_read",
                    "first canary read observed the bounded stale pointer",
                    &["canary.stale"],
                ))
            }
            OperationStatus::Running => {
                let converged = self.state.latest_version.clone();
                self.state.canary_reads.push(converged);
                operation.status = OperationStatus::Succeeded;
                Ok(event(
                    "canary.converged",
                    "second canary read observed the promoted exact version",
                    &["canary.converged"],
                ))
            }
            _ => bail!(
                "canary cannot advance operation in status {:?}",
                operation.status
            ),
        }
    }

    pub fn evaluate(&self) -> ReleaseRecoveryGates {
        let all_assets = self.state.run.published_assets == self.state.run.required_assets;
        let fresh = self.state.fresh_operation.as_ref();
        let canary_sequence = self.state.canary_reads.len() == 2
            && self.state.canary_reads[0] != self.state.tag.version
            && self.state.canary_reads[1] == self.state.tag.version;
        ReleaseRecoveryGates {
            immutable_identity: self.state.tag == self.state.original_tag,
            same_run_attempt_two: self.state.run.attempt == 2
                && self.state.run.status == RunStatus::Succeeded,
            exact_publication: self.state.exact_version_published && all_assets,
            stale_operation_rejected: self.state.stale_operation.status
                == OperationStatus::Rejected,
            one_fresh_gated_operation: fresh
                .is_some_and(|operation| operation.expected_latest.is_some()),
            one_latest_cas: self.state.latest_cas_count == 1,
            stale_then_converged_canary: canary_sequence,
            converged_latest: self.state.latest_version == self.state.tag.version
                && fresh.is_some_and(|operation| operation.status == OperationStatus::Succeeded),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRecoveryGates {
    pub immutable_identity: bool,
    pub same_run_attempt_two: bool,
    pub exact_publication: bool,
    pub stale_operation_rejected: bool,
    pub one_fresh_gated_operation: bool,
    pub one_latest_cas: bool,
    pub stale_then_converged_canary: bool,
    pub converged_latest: bool,
}

impl ReleaseRecoveryGates {
    pub fn passed(&self) -> bool {
        self.immutable_identity
            && self.same_run_attempt_two
            && self.exact_publication
            && self.stale_operation_rejected
            && self.one_fresh_gated_operation
            && self.one_latest_cas
            && self.stale_then_converged_canary
            && self.converged_latest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowOutcome {
    Advisory,
    NotEvaluated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShadowSnapshot {
    pub github_run_id: u64,
    pub github_run_attempt: u32,
    pub tag: String,
    pub exact_registry_version: String,
    pub registry_latest: String,
    pub release_operation_id: String,
    pub workers_observed_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShadowEvidence {
    pub outcome: ShadowOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ShadowSnapshot>,
    pub summary: String,
}

pub fn load_shadow_evidence(path: Option<&Path>) -> Result<ShadowEvidence> {
    let Some(path) = path else {
        return Ok(ShadowEvidence {
            outcome: ShadowOutcome::NotEvaluated,
            content_sha256: None,
            snapshot: None,
            summary: "read-only release shadow snapshot was not configured".into(),
        });
    };
    let bytes =
        fs::read(path).with_context(|| format!("read shadow snapshot {}", path.display()))?;
    let snapshot = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse shadow snapshot {}", path.display()))?;
    Ok(ShadowEvidence {
        outcome: ShadowOutcome::Advisory,
        content_sha256: Some(hex_digest(&bytes)),
        snapshot: Some(snapshot),
        summary: "environment-owned snapshot loaded as read-only advisory evidence".into(),
    })
}

fn validate_fixture(fixture: &ReleaseFixture) -> Result<()> {
    if fixture.tag.name.trim().is_empty()
        || fixture.tag.version.trim().is_empty()
        || !is_sha256(&fixture.tag.digest)
    {
        bail!("release fixture immutable tag is invalid");
    }
    if fixture.run.attempt != 1 || fixture.run.status != RunStatus::Cancelled {
        bail!("release fixture must begin at cancelled attempt 1");
    }
    if fixture.run.required_assets.is_empty()
        || fixture.run.published_assets.is_empty()
        || fixture.run.published_assets == fixture.run.required_assets
        || !fixture
            .run
            .published_assets
            .is_subset(&fixture.run.required_assets)
    {
        bail!("release fixture must contain a strict partial publication");
    }
    if fixture.exact_version_published || fixture.latest_graph_compatible {
        bail!("release fixture must begin unpublished with an incompatible latest graph");
    }
    if fixture.stale_operation.expected_latest.is_some()
        || fixture.stale_operation.status != OperationStatus::Pending
        || fixture.stale_operation.target_version != fixture.tag.version
    {
        bail!("release fixture stale operation must be pending with null expected_latest");
    }
    Ok(())
}

fn require_publication_complete(state: &ReleaseTrainState) -> Result<()> {
    if state.run.status != RunStatus::Succeeded
        || state.run.published_assets != state.run.required_assets
        || !state.exact_version_published
    {
        bail!("promotion preview requires completed assets and exact Registry publication");
    }
    Ok(())
}

fn event(id: &str, summary: &str, evidence_ids: &[&str]) -> SimulationEvent {
    SimulationEvent {
        id: id.into(),
        summary: summary.into(),
        evidence_ids: evidence_ids.iter().map(|value| (*value).into()).collect(),
    }
}

fn canonical_sha256(value: &impl Serialize) -> Result<String> {
    Ok(hex_digest(&serde_json::to_vec(value)?))
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
