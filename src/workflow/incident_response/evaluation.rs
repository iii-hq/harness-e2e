use std::collections::{BTreeSet, HashSet};

use super::helpers::{allowed_production_path, bounded_text, MAX_PATCH_BYTES};
use super::schemas::{AnalysisResult, DiagnosisResult, ReconcileResponse, ValidateResponse};

pub(super) fn validate_analysis(
    result: &AnalysisResult,
    expected_kind: &str,
    allowed_evidence: &BTreeSet<String>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if result.analysis_kind != expected_kind {
        failures.push(format!(
            "analysis_kind '{}' differs from expected '{expected_kind}'",
            result.analysis_kind
        ));
    }
    if result.hypotheses.is_empty() || result.hypotheses.len() > 12 {
        failures.push("hypotheses must contain 1..=12 entries".into());
    }
    let mut ids = HashSet::new();
    for hypothesis in &result.hypotheses {
        if hypothesis.id.is_empty()
            || hypothesis.id.len() > 64
            || !hypothesis
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || !ids.insert(hypothesis.id.as_str())
        {
            failures.push(format!(
                "invalid or duplicate hypothesis id '{}'",
                hypothesis.id
            ));
        }
        if !bounded_text(&hypothesis.claim, 2_000)
            || !bounded_text(&hypothesis.falsification_probe, 1_000)
            || hypothesis.observations.is_empty()
            || hypothesis
                .observations
                .iter()
                .any(|value| !bounded_text(value, 1_000))
        {
            failures.push(format!(
                "hypothesis '{}' contains invalid bounded text",
                hypothesis.id
            ));
        }
        if hypothesis.evidence_ids.is_empty()
            || hypothesis
                .evidence_ids
                .iter()
                .any(|id| !allowed_evidence.contains(id))
        {
            failures.push(format!(
                "hypothesis '{}' cites unknown evidence",
                hypothesis.id
            ));
        }
        if !hypothesis.confidence.is_finite() || !(0.0..=1.0).contains(&hypothesis.confidence) {
            failures.push(format!(
                "hypothesis '{}' confidence is outside 0..=1",
                hypothesis.id
            ));
        }
    }
    if result
        .limitations
        .iter()
        .any(|value| !bounded_text(value, 1_000))
    {
        failures.push("analysis limitations contain invalid bounded text".into());
    }
    failures
}

pub(super) fn validate_diagnosis(
    diagnosis: &DiagnosisResult,
    triage: &[AnalysisResult],
    allowed_evidence: &BTreeSet<String>,
) -> Vec<String> {
    let hypothesis_ids = triage
        .iter()
        .flat_map(|analysis| analysis.hypotheses.iter())
        .map(|hypothesis| hypothesis.id.as_str())
        .collect::<HashSet<_>>();
    let mut failures = Vec::new();
    if diagnosis.ranked_hypothesis_ids.is_empty()
        || diagnosis
            .ranked_hypothesis_ids
            .iter()
            .any(|id| !hypothesis_ids.contains(id.as_str()))
    {
        failures.push("ranked hypotheses are empty or contain unknown ids".into());
    }
    if !hypothesis_ids.contains(diagnosis.selected_hypothesis_id.as_str()) {
        failures.push("selected hypothesis is not present in validated triage".into());
    }
    if diagnosis.supporting_evidence_ids.is_empty()
        || diagnosis
            .supporting_evidence_ids
            .iter()
            .chain(diagnosis.contradicting_evidence_ids.iter())
            .any(|id| !allowed_evidence.contains(id))
    {
        failures.push("diagnosis cites missing or unknown evidence".into());
    }
    if diagnosis.causal_chain.len() < 3
        || diagnosis.causal_chain.len() > 12
        || diagnosis
            .causal_chain
            .iter()
            .any(|step| !bounded_text(step, 1_000))
    {
        failures.push("causal_chain must contain 3..=12 bounded steps".into());
    }
    if diagnosis.falsification_probe_ids.is_empty()
        || diagnosis.falsification_probe_ids.len() > 8
        || diagnosis
            .falsification_probe_ids
            .iter()
            .any(|probe| probe.is_empty() || probe.len() > 64)
    {
        failures.push("falsification_probe_ids are missing or invalid".into());
    }
    failures
}

pub(super) fn candidate_gate_vector(validation: &ValidateResponse) -> Vec<(&'static str, bool)> {
    let probes = |id: &str| {
        validation
            .probes
            .get(id)
            .is_some_and(|result| result.passed)
    };
    vec![
        (
            "patch_produced",
            !validation.patch.trim().is_empty() && validation.patch.len() <= MAX_PATCH_BYTES,
        ),
        (
            "allowed_paths_only",
            !validation.changed_paths.is_empty()
                && validation
                    .changed_paths
                    .iter()
                    .all(|path| allowed_production_path(path)),
        ),
        (
            "protected_paths_unchanged",
            validation.protected_paths_unchanged,
        ),
        ("tests_unchanged", validation.tests_unchanged),
        (
            "fixture_contract_unchanged",
            validation.fixture_contract_unchanged,
        ),
        (
            "working_tree_contains_only_candidate_change",
            validation.working_tree_candidate_only,
        ),
        ("focused_tests_passed", probes("focused_tests")),
        ("duplicate_delivery_safe", probes("duplicate_delivery")),
        ("concurrent_duplicate_safe", probes("concurrent_duplicate")),
        ("ack_timeout_replay_safe", probes("ack_timeout_replay")),
        ("distinct_events_preserved", probes("distinct_events")),
        ("ledger_invariant_restored", probes("ledger_invariant")),
        ("audit_history_preserved", probes("audit_history")),
        ("full_regression_passed", probes("full_regression")),
        ("canary_budget_passed", probes("canary_budget")),
        (
            "repair_round_budget_passed",
            validation.repair_rounds > 0
                && validation.repair_rounds
                    <= crate::scenarios::incident_response::MAX_REPAIR_ROUNDS,
        ),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TerminalDecision {
    pub should_promote: bool,
    pub should_rollback: bool,
    pub reason: String,
}

pub(super) fn decide(
    diagnosis_ready: bool,
    validation: Option<&ValidateResponse>,
) -> TerminalDecision {
    let candidate_valid = validation
        .map(candidate_gate_vector)
        .is_some_and(|gates| gates.iter().all(|(_, passed)| *passed))
        && validation
            .and_then(|value| value.candidate_sha.as_deref())
            .is_some();
    let should_promote = diagnosis_ready && candidate_valid;
    TerminalDecision {
        should_promote,
        should_rollback: !should_promote,
        reason: if should_promote {
            "diagnosis and every deterministic candidate gate passed".into()
        } else if !diagnosis_ready {
            "diagnosis did not authorize remediation".into()
        } else {
            "candidate validation was absent or at least one deterministic gate failed".into()
        },
    }
}

pub(super) fn final_reconciliation_passes(
    response: &ReconcileResponse,
    expected_revision: &str,
    promoted: bool,
) -> bool {
    response.deployed_revision == expected_revision
        && response.event_id == crate::scenarios::incident_response::INCIDENT_EVENT_ID
        && response.settlement_count <= 1
        && response.distinct_events_preserved
        && response.audit_history_preserved
        && response.active_operations == 0
        && if promoted {
            response.incident_status == "resolved"
        } else {
            matches!(
                response.incident_status.as_str(),
                "mitigated" | "rolled_back"
            )
        }
}
