use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::{self, ArtifactReference};
use crate::assessment::{
    AssessmentKind, AssessmentOutcome, AssessmentPolicy, AssessmentResult, AssessmentSource,
    AssessmentTarget, AssessmentTargetKind, AssetAssessmentResult, AssetValidationOutcome,
    AssetValidationResult, EvidenceReference,
};
use crate::redaction::{RedactionPolicy, RedactionReport};
use crate::report::{DeliverableReport, EvaluationDimension};
use crate::scenarios::{CapturedDeliverable, ProvenanceEvidence, ScenarioCase};

pub const ASSET_CAPTURE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_MAX_CAPTURED_ASSETS: usize = 64;
pub const DEFAULT_MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_PREVIEW_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssetCaptureLimits {
    pub max_assets: usize,
    pub max_total_bytes: u64,
    pub max_preview_bytes: usize,
}

impl Default for AssetCaptureLimits {
    fn default() -> Self {
        Self {
            max_assets: DEFAULT_MAX_CAPTURED_ASSETS,
            max_total_bytes: DEFAULT_MAX_CAPTURE_BYTES,
            max_preview_bytes: DEFAULT_MAX_PREVIEW_BYTES,
        }
    }
}

impl AssetCaptureLimits {
    fn validate(self) -> Result<Self> {
        if self.max_assets == 0 || self.max_total_bytes == 0 || self.max_preview_bytes == 0 {
            bail!("asset capture limits must be positive");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssetEvidenceEntry {
    pub asset_id: String,
    pub expected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<ProvenanceEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Value>,
    pub preview_truncated: bool,
    pub validation: AssetValidationResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssetCaptureManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub attempt_id: String,
    pub captured_at: String,
    pub captured_before_cleanup: bool,
    pub reconciled_after_cleanup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_capture: Option<ArtifactReference>,
    pub limits: AssetCaptureLimits,
    pub assets: Vec<AssetEvidenceEntry>,
}

#[derive(Debug)]
pub struct AssetCaptureEvaluation {
    pub deliverables: Vec<DeliverableReport>,
    pub assessments: Vec<AssetAssessmentResult>,
    pub redaction: RedactionReport,
    inventory: Vec<PendingAssetEvidence>,
    limits: AssetCaptureLimits,
}

#[derive(Debug)]
struct PendingAssetEvidence {
    asset_id: String,
    expected: bool,
    kind: Option<String>,
    media_type: Option<String>,
    observed_size_bytes: Option<u64>,
    preview: Option<Value>,
    preview_truncated: bool,
    provenance: Vec<ProvenanceEvidence>,
    persisted: bool,
}

pub fn evaluate_assets(
    case: &ScenarioCase,
    captured: Vec<CapturedDeliverable>,
    limits: AssetCaptureLimits,
) -> Result<AssetCaptureEvaluation> {
    evaluate_assets_with_policy(case, captured, limits, RedactionPolicy::from_environment())
}

fn evaluate_assets_with_policy(
    case: &ScenarioCase,
    captured: Vec<CapturedDeliverable>,
    limits: AssetCaptureLimits,
    policy: RedactionPolicy,
) -> Result<AssetCaptureEvaluation> {
    let limits = limits.validate()?;
    if case.deliverable_contract.artifacts.len() > limits.max_assets {
        bail!(
            "scenario '{}' declares {} assets; capture limit is {}",
            case.scenario_id,
            case.deliverable_contract.artifacts.len(),
            limits.max_assets
        );
    }

    let mut redaction = RedactionReport::default();
    let mut deliverables = Vec::new();
    let mut assessments = Vec::new();
    let mut inventory = Vec::new();
    let mut observed_ids = HashSet::new();
    let mut observed_invariants = HashSet::new();
    let expected_invariants = case
        .deliverable_contract
        .invariants
        .iter()
        .map(|invariant| invariant.id.as_str())
        .collect::<HashSet<_>>();
    let expectations = case
        .deliverable_contract
        .artifacts
        .iter()
        .map(|artifact| (artifact.id.as_str(), artifact))
        .collect::<HashMap<_, _>>();
    let mut total_bytes = 0_u64;

    let captured_count = captured.len();
    for mut candidate in captured.into_iter().take(limits.max_assets) {
        let expected = expectations.get(candidate.id.as_str()).copied();
        let expected_asset = expected.is_some();
        let raw_bytes = serde_json::to_vec(&candidate.content)
            .with_context(|| format!("serialize captured asset '{}'", candidate.id))?;
        let observed_size = u64::try_from(raw_bytes.len()).unwrap_or(u64::MAX);

        let (sanitized_id, id_redaction) = policy.redact_text(&candidate.id);
        if id_redaction.changed() {
            redaction.merge(id_redaction);
            push_validation(
                &mut assessments,
                &mut inventory,
                AssetValidationResult {
                    asset_id: sanitized_id.clone(),
                    outcome: AssetValidationOutcome::UnsafePath,
                    summary: "Captured asset id contained sensitive data and was rejected before path derivation."
                        .into(),
                    evidence: Vec::new(),
                },
                PendingAssetEvidence::rejected(
                    sanitized_id,
                    expected_asset,
                    None,
                    expected.map(|asset| asset.media_type.clone()),
                    Some(observed_size),
                ),
            );
            continue;
        }
        let observed_kind = candidate.kind.clone();
        let (kind, kind_redaction) = policy.redact_text(&candidate.kind);
        candidate.kind = kind;
        redaction.merge(kind_redaction);

        if !safe_asset_id(&candidate.id) {
            let summary = sanitized_summary(
                &policy,
                &mut redaction,
                &format!(
                    "Captured asset id '{}' is outside the safe path scope.",
                    candidate.id
                ),
            );
            push_validation(
                &mut assessments,
                &mut inventory,
                AssetValidationResult {
                    asset_id: candidate.id.clone(),
                    outcome: AssetValidationOutcome::UnsafePath,
                    summary,
                    evidence: Vec::new(),
                },
                PendingAssetEvidence {
                    asset_id: candidate.id,
                    expected: expected_asset,
                    kind: Some(candidate.kind),
                    media_type: expected.map(|asset| asset.media_type.clone()),
                    observed_size_bytes: Some(observed_size),
                    preview: None,
                    preview_truncated: false,
                    provenance: Vec::new(),
                    persisted: false,
                },
            );
            continue;
        }

        if !observed_ids.insert(candidate.id.clone()) {
            push_simple_validation(
                &policy,
                &mut redaction,
                &mut assessments,
                &mut inventory,
                PendingAssetEvidence::rejected(
                    candidate.id,
                    expected_asset,
                    Some(candidate.kind),
                    expected.map(|asset| asset.media_type.clone()),
                    Some(observed_size),
                ),
                AssetValidationOutcome::Invalid,
                "The asset was captured more than once; duplicate content was not persisted.",
            );
            continue;
        }

        if total_bytes.saturating_add(observed_size) > limits.max_total_bytes
            || expected.is_some_and(|asset| observed_size > asset.max_size_bytes)
        {
            let allowed = expected
                .map(|asset| asset.max_size_bytes.min(limits.max_total_bytes))
                .unwrap_or(limits.max_total_bytes);
            push_simple_validation(
                &policy,
                &mut redaction,
                &mut assessments,
                &mut inventory,
                PendingAssetEvidence::rejected(
                    candidate.id,
                    expected_asset,
                    Some(candidate.kind),
                    expected.map(|asset| asset.media_type.clone()),
                    Some(observed_size),
                ),
                AssetValidationOutcome::Oversized,
                &format!(
                    "Captured asset is {observed_size} bytes; bounded capture allows at most {allowed} bytes."
                ),
            );
            continue;
        }
        total_bytes = total_bytes.saturating_add(observed_size);

        let mut outcome = if expected.is_some() {
            AssetValidationOutcome::Valid
        } else {
            AssetValidationOutcome::Unexpected
        };
        let mut reasons = Vec::new();
        let mut schema_valid = false;
        let mut provenance_valid =
            !case.deliverable_contract.provenance_required || !candidate.provenance.is_empty();

        if let Some(expectation) = expected {
            if observed_kind != expectation.kind {
                outcome = AssetValidationOutcome::Malformed;
                reasons.push(format!(
                    "kind '{}' differs from expected '{}'",
                    observed_kind, expectation.kind
                ));
            }
            let validator =
                jsonschema::JSONSchema::compile(&expectation.schema).map_err(|error| {
                    anyhow::anyhow!(
                        "compile asset '{}' JSON Schema for scenario '{}': {error}",
                        candidate.id,
                        case.scenario_id
                    )
                })?;
            schema_valid = validator.is_valid(&candidate.content);
            if !schema_valid {
                outcome = AssetValidationOutcome::Malformed;
                reasons.push("content does not match the declared JSON Schema".into());
            }
        } else {
            reasons.push("asset is not declared by the scenario contract".into());
        }

        if candidate.provenance.iter().any(|evidence| {
            evidence.kind.trim().is_empty()
                || evidence.source_id.trim().is_empty()
                || evidence.relation.trim().is_empty()
        }) {
            provenance_valid = false;
            if outcome == AssetValidationOutcome::Valid {
                outcome = AssetValidationOutcome::Invalid;
            }
            reasons.push("provenance metadata is incomplete".into());
        } else if !provenance_valid {
            if outcome == AssetValidationOutcome::Valid {
                outcome = AssetValidationOutcome::Invalid;
            }
            reasons.push("required provenance is missing".into());
        }

        redaction.merge(policy.redact_value(&mut candidate.content));
        for invariant in &mut candidate.invariants {
            let observed_invariant_id = invariant.id.clone();
            let (invariant_id, nested) = policy.redact_text(&invariant.id);
            invariant.id = invariant_id;
            redaction.merge(nested);
            let (reason, nested) = policy.redact_text(&invariant.reason);
            invariant.reason = reason;
            redaction.merge(nested);
            if !expected_invariants.contains(observed_invariant_id.as_str()) {
                if outcome == AssetValidationOutcome::Valid {
                    outcome = AssetValidationOutcome::Invalid;
                }
                reasons.push(format!(
                    "invariant '{}' is not declared",
                    observed_invariant_id
                ));
            } else if !observed_invariants.insert(observed_invariant_id.clone()) {
                if outcome == AssetValidationOutcome::Valid {
                    outcome = AssetValidationOutcome::Invalid;
                }
                reasons.push(format!(
                    "invariant '{}' was reported more than once",
                    observed_invariant_id
                ));
            }
            if !invariant.passed {
                if outcome == AssetValidationOutcome::Valid {
                    outcome = AssetValidationOutcome::Invalid;
                }
                reasons.push(format!(
                    "invariant '{}' failed: {}",
                    observed_invariant_id, invariant.reason
                ));
            }
        }
        for provenance in &mut candidate.provenance {
            for value in [
                &mut provenance.kind,
                &mut provenance.source_id,
                &mut provenance.relation,
            ] {
                let (sanitized, nested) = policy.redact_text(value);
                *value = sanitized;
                redaction.merge(nested);
            }
        }

        let sanitized_bytes = serde_json::to_vec(&candidate.content)
            .with_context(|| format!("serialize sanitized asset '{}'", candidate.id))?;
        let content_size_bytes = u64::try_from(sanitized_bytes.len()).unwrap_or(u64::MAX);
        let (preview, preview_truncated) =
            bounded_preview(&candidate.content, limits.max_preview_bytes);

        let summary = if outcome == AssetValidationOutcome::Valid {
            "Asset passed deterministic type, schema, size, provenance, and invariant checks."
                .to_string()
        } else {
            reasons.join("; ")
        };
        let summary = sanitized_summary(&policy, &mut redaction, &summary);
        let report = DeliverableReport {
            id: candidate.id.clone(),
            kind: candidate.kind.clone(),
            media_type: expected
                .map(|asset| asset.media_type.clone())
                .unwrap_or_else(|| "application/json".into()),
            content_sha256: artifact::sha256_value(&candidate.content)?,
            content_size_bytes,
            schema_valid,
            provenance_valid,
            invariants: candidate.invariants,
            provenance: candidate.provenance.clone(),
            preview: preview.clone(),
            artifact: None,
            content: candidate.content,
        };
        push_validation(
            &mut assessments,
            &mut inventory,
            AssetValidationResult {
                asset_id: candidate.id.clone(),
                outcome,
                summary,
                evidence: Vec::new(),
            },
            PendingAssetEvidence {
                asset_id: candidate.id,
                expected: expected_asset,
                kind: Some(candidate.kind),
                media_type: Some(report.media_type.clone()),
                observed_size_bytes: Some(observed_size),
                preview: Some(preview),
                preview_truncated,
                provenance: candidate.provenance,
                persisted: true,
            },
        );
        deliverables.push(report);
    }

    if captured_count > limits.max_assets {
        push_simple_validation(
            &policy,
            &mut redaction,
            &mut assessments,
            &mut inventory,
            PendingAssetEvidence::rejected("capture_inventory".into(), false, None, None, None),
            AssetValidationOutcome::Oversized,
            &format!(
                "Capture returned {captured_count} assets; only {} bounded entries were inspected.",
                limits.max_assets
            ),
        );
    }

    for expectation in &case.deliverable_contract.artifacts {
        if !observed_ids.contains(&expectation.id) {
            push_simple_validation(
                &policy,
                &mut redaction,
                &mut assessments,
                &mut inventory,
                PendingAssetEvidence::rejected(
                    expectation.id.clone(),
                    true,
                    Some(expectation.kind.clone()),
                    Some(expectation.media_type.clone()),
                    None,
                ),
                AssetValidationOutcome::NotProduced,
                "Expected asset was not produced.",
            );
        }
    }

    let missing_invariants = expected_invariants
        .difference(
            &observed_invariants
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
        )
        .copied()
        .collect::<Vec<_>>();
    if !missing_invariants.is_empty() {
        push_simple_validation(
            &policy,
            &mut redaction,
            &mut assessments,
            &mut inventory,
            PendingAssetEvidence::rejected("capture_invariants".into(), true, None, None, None),
            AssetValidationOutcome::Invalid,
            &format!(
                "Captured assets did not report invariants: {}.",
                missing_invariants.join(", ")
            ),
        );
    }

    Ok(AssetCaptureEvaluation {
        deliverables,
        assessments,
        redaction,
        inventory,
        limits,
    })
}

pub fn failed_capture_evaluation(
    case: &ScenarioCase,
    outcome: AssetValidationOutcome,
    reason: &str,
) -> AssetCaptureEvaluation {
    let policy = RedactionPolicy::from_environment();
    let mut redaction = RedactionReport::default();
    let reason = sanitized_summary(&policy, &mut redaction, reason);
    let mut assessments = Vec::new();
    let mut inventory = Vec::new();
    for expectation in &case.deliverable_contract.artifacts {
        push_validation(
            &mut assessments,
            &mut inventory,
            AssetValidationResult {
                asset_id: expectation.id.clone(),
                outcome,
                summary: reason.clone(),
                evidence: Vec::new(),
            },
            PendingAssetEvidence {
                asset_id: expectation.id.clone(),
                expected: true,
                kind: Some(expectation.kind.clone()),
                media_type: Some(expectation.media_type.clone()),
                observed_size_bytes: None,
                preview: None,
                preview_truncated: false,
                provenance: Vec::new(),
                persisted: false,
            },
        );
    }
    AssetCaptureEvaluation {
        deliverables: Vec::new(),
        assessments,
        redaction,
        inventory,
        limits: AssetCaptureLimits::default(),
    }
}

pub fn persist_before_cleanup(
    output: &Path,
    run_id: &str,
    attempt_id: &str,
    evaluation: &mut AssetCaptureEvaluation,
) -> Result<ArtifactReference> {
    let deliverable_root = PathBuf::from("deliverables").join(run_id).join(attempt_id);
    for report in &mut evaluation.deliverables {
        let reference = artifact::write_json(
            output,
            &deliverable_root.join(format!("{}.json", report.id)),
            report.id.clone(),
            report.kind.clone(),
            &report.content,
        )?;
        report.artifact = Some(reference);
    }

    for (assessment, pending) in evaluation.assessments.iter_mut().zip(&evaluation.inventory) {
        if let Some(reference) = pending
            .persisted
            .then(|| {
                evaluation
                    .deliverables
                    .iter()
                    .find(|report| report.id == assessment.validation.asset_id)
            })
            .flatten()
            .and_then(|report| report.artifact.as_ref())
        {
            assessment.validation.evidence = vec![EvidenceReference {
                artifact_id: reference.id.clone(),
                artifact_sha256: reference.sha256.clone(),
                locator: None,
            }];
        }
    }

    let assets = evaluation
        .inventory
        .iter()
        .zip(&evaluation.assessments)
        .map(|(pending, validation)| {
            let report = pending
                .persisted
                .then(|| {
                    evaluation
                        .deliverables
                        .iter()
                        .find(|report| report.id == pending.asset_id)
                })
                .flatten();
            Ok(AssetEvidenceEntry {
                asset_id: pending.asset_id.clone(),
                expected: pending.expected,
                kind: pending.kind.clone(),
                media_type: pending.media_type.clone(),
                observed_size_bytes: pending.observed_size_bytes,
                content_sha256: report.map(|report| report.content_sha256.clone()),
                normalized_path: report
                    .and_then(|report| report.artifact.as_ref())
                    .map(|artifact| artifact.path.clone()),
                artifact: report.and_then(|report| report.artifact.clone()),
                provenance: pending.provenance.clone(),
                preview: pending.preview.clone(),
                preview_truncated: pending.preview_truncated,
                validation: validation.validation.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = AssetCaptureManifest {
        schema_version: ASSET_CAPTURE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        attempt_id: attempt_id.to_string(),
        captured_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        captured_before_cleanup: true,
        reconciled_after_cleanup: false,
        prior_capture: None,
        limits: evaluation.limits,
        assets,
    };
    artifact::write_json(
        output,
        &PathBuf::from("evidence")
            .join(run_id)
            .join(attempt_id)
            .join("asset-capture-v1.json"),
        "asset_capture",
        "asset_capture_manifest",
        &manifest,
    )
}

pub fn reconcile_after_cleanup(
    output: &Path,
    deliverables: &[DeliverableReport],
    assessments: &mut [AssetAssessmentResult],
) {
    for deliverable in deliverables {
        let Some(reference) = &deliverable.artifact else {
            continue;
        };
        if reference.verify(output).is_ok() {
            continue;
        }
        if let Some(assessment) = assessments
            .iter_mut()
            .find(|assessment| assessment.validation.asset_id == deliverable.id)
        {
            assessment.validation.outcome = AssetValidationOutcome::RemovedDuringCleanup;
            assessment.validation.summary =
                "Captured evidence was removed during cleanup and will be restored from bounded in-memory content."
                    .into();
        }
    }
}

pub fn persist_after_cleanup(
    output: &Path,
    capture_manifest: &ArtifactReference,
    assessments: &[AssetAssessmentResult],
) -> Result<ArtifactReference> {
    capture_manifest.verify(output)?;
    let bytes = fs::read(output.join(&capture_manifest.path)).with_context(|| {
        format!(
            "read pre-cleanup asset capture manifest {}",
            capture_manifest.path
        )
    })?;
    let mut manifest: AssetCaptureManifest =
        serde_json::from_slice(&bytes).context("decode pre-cleanup asset capture manifest")?;
    if manifest.assets.len() != assessments.len() {
        bail!(
            "asset reconciliation count differs from pre-cleanup capture: {} != {}",
            assessments.len(),
            manifest.assets.len()
        );
    }
    for (asset, assessment) in manifest.assets.iter_mut().zip(assessments) {
        if asset.asset_id != assessment.validation.asset_id {
            bail!(
                "asset reconciliation order differs at '{}' != '{}'",
                asset.asset_id,
                assessment.validation.asset_id
            );
        }
        asset.validation = assessment.validation.clone();
    }
    manifest.reconciled_after_cleanup = true;
    manifest.prior_capture = Some(capture_manifest.clone());
    let root = Path::new(&capture_manifest.path)
        .parent()
        .context("pre-cleanup asset capture manifest has no parent directory")?;
    artifact::write_json(
        output,
        &root.join("asset-reconciliation-v1.json"),
        "asset_reconciliation",
        "asset_capture_manifest",
        &manifest,
    )
}

fn push_simple_validation(
    policy: &RedactionPolicy,
    redaction: &mut RedactionReport,
    assessments: &mut Vec<AssetAssessmentResult>,
    inventory: &mut Vec<PendingAssetEvidence>,
    pending: PendingAssetEvidence,
    outcome: AssetValidationOutcome,
    summary: &str,
) {
    let summary = sanitized_summary(policy, redaction, summary);
    push_validation(
        assessments,
        inventory,
        AssetValidationResult {
            asset_id: pending.asset_id.clone(),
            outcome,
            summary,
            evidence: Vec::new(),
        },
        pending,
    );
}

impl PendingAssetEvidence {
    fn rejected(
        asset_id: String,
        expected: bool,
        kind: Option<String>,
        media_type: Option<String>,
        observed_size_bytes: Option<u64>,
    ) -> Self {
        Self {
            asset_id,
            expected,
            kind,
            media_type,
            observed_size_bytes,
            preview: None,
            preview_truncated: false,
            provenance: Vec::new(),
            persisted: false,
        }
    }
}

fn push_validation(
    assessments: &mut Vec<AssetAssessmentResult>,
    inventory: &mut Vec<PendingAssetEvidence>,
    validation: AssetValidationResult,
    pending: PendingAssetEvidence,
) {
    assessments.push(asset_assessment(validation));
    inventory.push(pending);
}

fn asset_assessment(validation: AssetValidationResult) -> AssetAssessmentResult {
    let asset_id = validation.asset_id.clone();
    AssetAssessmentResult {
        validation,
        qualitative_assessment: AssessmentResult {
            criterion_id: "asset_quality".into(),
            target: AssessmentTarget {
                kind: AssessmentTargetKind::Asset,
                id: asset_id,
            },
            kind: AssessmentKind::AssetQuality,
            policy: AssessmentPolicy::Advisory,
            dimension: EvaluationDimension::Deliverable,
            source: AssessmentSource::AssetAnalyzer,
            outcome: AssessmentOutcome::NotEvaluated,
            score: None,
            confidence: None,
            summary: "Qualitative asset assessment is not evaluated by deterministic capture."
                .into(),
            evidence: Vec::new(),
            analyzer: None,
            analyzer_usage: None,
        },
    }
}

fn sanitized_summary(
    policy: &RedactionPolicy,
    redaction: &mut RedactionReport,
    summary: &str,
) -> String {
    let (summary, nested) = policy.redact_text(summary);
    redaction.merge(nested);
    summary
}

fn bounded_preview(content: &Value, limit: usize) -> (Value, bool) {
    let rendered = serde_json::to_string(content).unwrap_or_default();
    if rendered.len() <= limit {
        return (content.clone(), false);
    }
    let mut end = limit.min(rendered.len());
    while !rendered.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (Value::String(format!("{}...", &rendered[..end])), true)
}

fn safe_asset_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::{
        ArtifactExpectation, CapturedInvariant, ComplexityProfile, DeliverableContract,
        InvariantSpec,
    };

    fn case(max_size_bytes: u64) -> ScenarioCase {
        ScenarioCase::new(
            "asset_capture",
            1,
            7,
            serde_json::json!({}),
            ComplexityProfile::default(),
            vec![],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: "result".into(),
                    kind: "json".into(),
                    media_type: "application/json".into(),
                    schema: serde_json::json!({"type": "object"}),
                    max_size_bytes,
                }],
                invariants: vec![InvariantSpec {
                    id: "correct".into(),
                    description: "The result is correct.".into(),
                }],
                provenance_required: true,
                capture_before_cleanup: true,
            },
        )
        .unwrap()
    }

    fn captured(id: &str, content: Value) -> CapturedDeliverable {
        CapturedDeliverable {
            id: id.into(),
            kind: "json".into(),
            content,
            invariants: vec![CapturedInvariant {
                id: "correct".into(),
                passed: true,
                reason: "correct".into(),
            }],
            provenance: vec![ProvenanceEvidence {
                kind: "function_call".into(),
                source_id: "call-1".into(),
                relation: "created".into(),
            }],
        }
    }

    #[test]
    fn inventory_distinguishes_missing_unexpected_unsafe_and_oversized_assets() {
        let missing = evaluate_assets(&case(100), vec![], AssetCaptureLimits::default()).unwrap();
        assert_eq!(
            missing.assessments[0].validation.outcome,
            AssetValidationOutcome::NotProduced
        );

        let unexpected = evaluate_assets(
            &case(100),
            vec![captured("extra", serde_json::json!({}))],
            AssetCaptureLimits::default(),
        )
        .unwrap();
        assert!(unexpected
            .assessments
            .iter()
            .any(|asset| asset.validation.outcome == AssetValidationOutcome::Unexpected));
        assert!(unexpected
            .assessments
            .iter()
            .any(|asset| asset.validation.outcome == AssetValidationOutcome::NotProduced));

        let unsafe_path = evaluate_assets(
            &case(100),
            vec![captured("../result", serde_json::json!({}))],
            AssetCaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(
            unsafe_path.assessments[0].validation.outcome,
            AssetValidationOutcome::UnsafePath
        );
        assert!(unsafe_path.deliverables.is_empty());

        let oversized = evaluate_assets(
            &case(2),
            vec![captured(
                "result",
                serde_json::json!({"value": "too large"}),
            )],
            AssetCaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(
            oversized.assessments[0].validation.outcome,
            AssetValidationOutcome::Oversized
        );
        assert!(oversized.deliverables.is_empty());
    }

    #[test]
    fn validation_distinguishes_malformed_content_from_invalid_evidence() {
        let malformed = evaluate_assets(
            &case(4_096),
            vec![captured("result", serde_json::json!("wrong shape"))],
            AssetCaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(
            malformed.assessments[0].validation.outcome,
            AssetValidationOutcome::Malformed
        );

        let mut invalid = captured("result", serde_json::json!({"value": "ok"}));
        invalid.invariants[0].passed = false;
        invalid.invariants[0].reason = "expected invariant failed".into();
        let invalid =
            evaluate_assets(&case(4_096), vec![invalid], AssetCaptureLimits::default()).unwrap();
        assert_eq!(
            invalid.assessments[0].validation.outcome,
            AssetValidationOutcome::Invalid
        );
        assert!(invalid.assessments[0]
            .validation
            .summary
            .contains("invariant 'correct' failed"));
    }

    #[test]
    fn sensitive_asset_ids_are_rejected_before_path_derivation() {
        let evaluation = evaluate_assets_with_policy(
            &case(4_096),
            vec![captured("knownsecretasset", serde_json::json!({}))],
            AssetCaptureLimits::default(),
            RedactionPolicy::with_known_values(["knownsecretasset".into()]),
        )
        .unwrap();

        assert_eq!(evaluation.assessments[0].validation.asset_id, "[REDACTED]");
        assert_eq!(
            evaluation.assessments[0].validation.outcome,
            AssetValidationOutcome::UnsafePath
        );
        assert!(evaluation.deliverables.is_empty());
        assert!(evaluation.redaction.changed());
    }

    #[test]
    fn validation_uses_original_content_but_persistence_uses_redacted_content() {
        let output = tempfile::tempdir().unwrap();
        let mut scenario = case(4_096);
        scenario.deliverable_contract.artifacts[0].schema = serde_json::json!({
            "type": "object",
            "required": ["password"],
            "properties": {"password": {"type": "number"}},
            "additionalProperties": false
        });
        let mut evaluation = evaluate_assets_with_policy(
            &scenario,
            vec![captured("result", serde_json::json!({"password": 7}))],
            AssetCaptureLimits::default(),
            RedactionPolicy::default(),
        )
        .unwrap();
        assert_eq!(
            evaluation.assessments[0].validation.outcome,
            AssetValidationOutcome::Valid
        );

        persist_before_cleanup(output.path(), "run", "attempt", &mut evaluation).unwrap();
        let persisted = std::fs::read_to_string(
            output
                .path()
                .join(&evaluation.deliverables[0].artifact.as_ref().unwrap().path),
        )
        .unwrap();
        assert!(persisted.contains("[REDACTED]"));
        assert!(!persisted.contains(": 7"));
    }

    #[test]
    fn capture_manifest_redacts_asset_metadata() {
        let output = tempfile::tempdir().unwrap();
        let secret = "knownsecretmetadata";
        let mut candidate = captured("result", serde_json::json!({"value": "ok"}));
        candidate.kind = secret.into();
        candidate.invariants[0].id = secret.into();
        candidate.invariants[0].reason = format!("reason includes {secret}");
        candidate.provenance[0].source_id = secret.into();
        let mut evaluation = evaluate_assets_with_policy(
            &case(4_096),
            vec![candidate],
            AssetCaptureLimits::default(),
            RedactionPolicy::with_known_values([secret.into()]),
        )
        .unwrap();
        let manifest =
            persist_before_cleanup(output.path(), "run", "attempt", &mut evaluation).unwrap();
        let manifest = std::fs::read_to_string(output.path().join(manifest.path)).unwrap();

        assert!(!manifest.contains(secret));
        assert!(manifest.contains("[REDACTED]"));
        assert!(evaluation.redaction.changed());
    }

    #[test]
    fn capture_is_persisted_with_bounded_redacted_preview_before_cleanup() {
        let output = tempfile::tempdir().unwrap();
        let mut evaluation = evaluate_assets(
            &case(4_096),
            vec![captured(
                "result",
                serde_json::json!({"password": "do-not-persist", "value": "x".repeat(2_000)}),
            )],
            AssetCaptureLimits::default(),
        )
        .unwrap();
        let manifest =
            persist_before_cleanup(output.path(), "run", "attempt", &mut evaluation).unwrap();

        assert!(output.path().join(&manifest.path).is_file());
        let asset = evaluation.deliverables[0].artifact.as_ref().unwrap();
        let bytes = std::fs::read(output.path().join(&asset.path)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("do-not-persist"));
        assert_eq!(evaluation.assessments[0].validation.evidence.len(), 1);
        let manifest: AssetCaptureManifest =
            serde_json::from_slice(&std::fs::read(output.path().join(manifest.path)).unwrap())
                .unwrap();
        assert!(manifest.captured_before_cleanup);
        assert!(!manifest.reconciled_after_cleanup);
        assert!(manifest.prior_capture.is_none());
        assert!(manifest.assets[0].preview_truncated);
    }

    #[test]
    fn unreadable_capture_is_persisted_as_a_structured_inventory() {
        let output = tempfile::tempdir().unwrap();
        let mut evaluation = failed_capture_evaluation(
            &case(4_096),
            AssetValidationOutcome::Unreadable,
            "storage returned unreadable bytes",
        );
        let manifest =
            persist_before_cleanup(output.path(), "run", "attempt", &mut evaluation).unwrap();
        let manifest: AssetCaptureManifest =
            serde_json::from_slice(&std::fs::read(output.path().join(manifest.path)).unwrap())
                .unwrap();

        assert_eq!(manifest.assets.len(), 1);
        assert_eq!(
            manifest.assets[0].validation.outcome,
            AssetValidationOutcome::Unreadable
        );
        assert!(manifest.assets[0].artifact.is_none());
        assert!(manifest.assets[0].validation.evidence.is_empty());
    }
}
