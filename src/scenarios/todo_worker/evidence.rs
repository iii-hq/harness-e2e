use super::workspace::validation_bundle_path;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Passed,
    Failed,
    NotEvaluated,
    InfrastructureError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeObservation {
    pub id: String,
    pub kind: String,
    pub expected: Value,
    pub observed: Value,
    pub outcome: ProbeOutcome,
    pub duration_ms: u64,
    pub repetition: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationAttempt {
    pub ordinal: u32,
    pub candidate_sha256: Option<String>,
    pub verdict: String,
    pub persisted_before_feedback: bool,
    pub probes: Vec<ProbeObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationCoverage {
    pub required: Vec<String>,
    pub covered: Vec<String>,
    pub omitted: Vec<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepeatabilityEvidence {
    pub planned: u8,
    pub completed: u8,
    pub passed: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidatorIdentity {
    pub id: String,
    pub contract_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidatedSubject {
    pub worker_name: String,
    pub candidate_sha256: Option<String>,
    pub source_sha256: Option<String>,
    pub manifest_sha256: Option<String>,
    pub accepted_candidate_sha256: Option<String>,
    pub accepted_candidate_is_current: bool,
    pub function_schema_sha256: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationEvidenceBundle {
    pub scenario_version: u32,
    pub contract_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_sha256: Option<String>,
    pub validator: ValidatorIdentity,
    pub subject: ValidatedSubject,
    pub coverage: ValidationCoverage,
    pub attempts: Vec<ValidationAttempt>,
    pub nudges: u32,
    pub repeatability: RepeatabilityEvidence,
    #[serde(default)]
    pub limitations: Vec<String>,
}

impl ValidationEvidenceBundle {
    pub fn probe_observations(&self, id: &str) -> Vec<&ProbeObservation> {
        self.attempts
            .iter()
            .flat_map(|attempt| &attempt.probes)
            .filter(|probe| probe.id == id)
            .collect()
    }

    pub fn probe_passed(&self, id: &str) -> bool {
        let observed = self
            .attempts
            .last()
            .into_iter()
            .flat_map(|attempt| &attempt.probes)
            .filter(|probe| probe.id == id)
            .collect::<Vec<_>>();
        !observed.is_empty()
            && observed
                .iter()
                .all(|probe| probe.outcome == ProbeOutcome::Passed)
    }

    pub fn evidence_complete(&self) -> bool {
        self.coverage.complete
            && self.subject.candidate_sha256.is_some()
            && self
                .attempts
                .iter()
                .all(|attempt| !attempt.probes.is_empty())
    }
}

pub(super) fn validation_deliverable(
    contract: &TodoTaskContract,
    bundle: ValidationEvidenceBundle,
    assessments: &[AssessmentSpec],
) -> CapturedDeliverable {
    let invariants = assessments
        .iter()
        .map(|assessment| {
            let passed = match assessment.id() {
                "evidence_complete" => bundle.evidence_complete(),
                "auditor_bound_once"
                | "validation_attempt_observed"
                | "attempts_persisted_before_feedback" => false,
                "accepted_candidate_is_current" => bundle.subject.accepted_candidate_is_current,
                "repeatability_3_of_3" => bundle.repeatability.passed == 3,
                id => bundle.probe_passed(id),
            };
            CapturedInvariant {
                id: assessment.id().into(),
                passed,
                reason: if passed {
                    "satisfied by the captured validation bundle".into()
                } else {
                    probe_reason(&bundle, assessment.id())
                },
            }
        })
        .collect();
    CapturedDeliverable {
        id: VALIDATION_ASSET_ID.into(),
        kind: "todo_validation_evidence".into(),
        content: serde_json::to_value(bundle)
            .expect("serialize validation bundle")
            .into(),
        invariants,
        provenance: vec![
            ProvenanceEvidence {
                kind: "worker".into(),
                source_id: contract.worker_name.clone(),
                relation: "independently_validated".into(),
            },
            ProvenanceEvidence {
                kind: "filesystem_path".into(),
                source_id: validation_bundle_path(contract).display().to_string(),
                relation: "persisted_before_cleanup".into(),
            },
        ],
    }
}

pub(super) fn validation_deliverable_contract(
    assessments: &[AssessmentSpec],
) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: VALIDATION_ASSET_ID.into(),
            kind: "todo_validation_evidence".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type": "object",
                "required": ["scenario_version", "contract_sha256", "validator", "subject", "coverage", "attempts", "nudges", "repeatability", "limitations"],
                "properties": {
                    "scenario_version": {"const": VERSION},
                    "contract_sha256": {"type": "string"},
                    "plan_sha256": {"type": "string"},
                    "validator": {"type": "object"},
                    "subject": {"type": "object"},
                    "coverage": {"type": "object"},
                    "attempts": {"type": "array"},
                    "nudges": {"type": "integer", "minimum": 0},
                    "repeatability": {"type": "object"},
                    "limitations": {"type": "array"}
                },
                "additionalProperties": false
            }),
            max_size_bytes: 256 * 1024,
        }],
        invariants: assessments
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().into(),
                description: assessment.description().into(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

pub(super) fn captured_bundle(
    observation: &ScenarioObservation,
) -> Result<ValidationEvidenceBundle> {
    let deliverable = observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == VALIDATION_ASSET_ID)
        .context("Todo validation deliverable is missing")?;
    let CapturedDeliverableContent::Json(value) = &deliverable.content else {
        bail!("Todo validation deliverable is not JSON");
    };
    serde_json::from_value(value.clone()).context("decode captured Todo validation bundle")
}

pub(super) fn probe_reason(bundle: &ValidationEvidenceBundle, id: &str) -> String {
    let observed = bundle.probe_observations(id);
    if observed.is_empty() {
        return format!("probe '{id}' was not observed");
    }
    observed
        .iter()
        .map(|probe| {
            format!(
                "repetition {}: {:?}; observed={}",
                probe.repetition,
                probe.outcome,
                bounded_value(&probe.observed)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) fn evidence_reason(bundle: &ValidationEvidenceBundle) -> String {
    format!(
        "coverage complete={}, attempts={}, candidate={:?}, limitations={:?}",
        bundle.coverage.complete,
        bundle.attempts.len(),
        bundle.subject.candidate_sha256,
        bundle.limitations
    )
}

pub(super) fn probe(
    id: &str,
    kind: &str,
    expected: Value,
    observed: Value,
    outcome: ProbeOutcome,
    started: Instant,
    repetition: u8,
) -> ProbeObservation {
    ProbeObservation {
        id: id.into(),
        kind: kind.into(),
        expected,
        observed: bounded_value(&observed),
        outcome,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        repetition,
    }
}

pub(super) fn outcome(passed: bool) -> ProbeOutcome {
    if passed {
        ProbeOutcome::Passed
    } else {
        ProbeOutcome::Failed
    }
}

pub(super) fn bounded_value(value: &Value) -> Value {
    let rendered = serde_json::to_string(value).unwrap_or_default();
    if rendered.len() <= 4_096 {
        value.clone()
    } else {
        json!({
            "omitted": true,
            "reason": "Todo validation observation exceeded 4096 bytes",
            "sha256": crate::artifact::sha256_value(value).ok()
        })
    }
}
