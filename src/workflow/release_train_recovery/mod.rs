use std::collections::BTreeSet;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod adaptive_runtime;

pub use adaptive_runtime::build_adaptive_runtime;

pub const SCENARIO_ID: &str = "release_train_recovery";
pub const INVALIDATION_EVIDENCE_ID: &str = "promotion_preview.incompatible_latest_graph";

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
    CreateFreshGatedOperation {
        expected_latest: String,
    },
    ObserveCanary,
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
