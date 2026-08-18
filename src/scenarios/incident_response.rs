//! Contract for the code-owned `incident_response` composite scenario.
//!
//! The executable graph and the environment-owned fixture functions live in
//! `workflow::incident_response`. This module deliberately owns only the
//! stable scenario identity, materialized case, deterministic assessment
//! declarations, and the domain-asset catalog. Composite workflow assets are
//! persisted by the workflow scheduler before its mandatory cleanup, so they
//! are not duplicated as ordinary `ScenarioDeliverableCapture` hooks here.

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    ComplexityProfile, CriterionSpec, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    MaterializedScenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "incident_response";
pub const VERSION: u32 = 1;
pub const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_INCIDENT_FIXTURE_PATH";
pub const KNOWN_GOOD_REF: &str = "refs/tags/known_good";
pub const INCIDENT_REF: &str = "refs/tags/incident";
pub const INCIDENT_EVENT_ID: &str = "evt-duplicate-42";
pub const MAX_REPAIR_ROUNDS: u8 = 2;

pub const EXPECTED_INVARIANTS: [&str; 5] = [
    "one_settlement_per_event",
    "distinct_events_preserved",
    "append_only_audit",
    "protected_paths_unchanged",
    "deploy_exact_validated_revision",
];

pub const ALLOWED_PATH_PATTERNS: [&str; 1] = ["src/**"];
pub const PROTECTED_PATH_PATTERNS: [&str; 4] = [
    "tests/**",
    "fixture_contract.json",
    ".harness-e2e/**",
    ".git/**",
];

pub const PUBLIC_PROBE_IDS: [&str; 2] = ["focused_settlement", "public_regression"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncidentAssetSpec {
    pub id: &'static str,
    pub producer_node: &'static str,
    pub kind: &'static str,
    pub media_type: &'static str,
}

pub const ASSETS: [IncidentAssetSpec; 11] = [
    IncidentAssetSpec {
        id: "baseline_snapshot",
        producer_node: "capture_baseline",
        kind: "incident_baseline",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "incident_record",
        producer_node: "deduplicate_alert",
        kind: "incident_record",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "reproduction_record",
        producer_node: "reproduce_incident",
        kind: "incident_reproduction",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "triage_bundle",
        producer_node: "validate_triage",
        kind: "incident_triage",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "diagnosis_record",
        producer_node: "validate_diagnosis",
        kind: "incident_diagnosis",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "remediation_patch",
        producer_node: "validate_candidate",
        kind: "code_patch",
        media_type: "text/x-diff; charset=utf-8",
    },
    IncidentAssetSpec {
        id: "change_manifest",
        producer_node: "validate_candidate",
        kind: "change_manifest",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "validation_matrix",
        producer_node: "validate_candidate",
        kind: "validation_matrix",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "decision_record",
        producer_node: "decide_terminal_action",
        kind: "incident_decision",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "final_state",
        producer_node: "reconcile_final_state",
        kind: "incident_final_state",
        media_type: "application/json",
    },
    IncidentAssetSpec {
        id: "incident_report",
        producer_node: "validate_incident_report",
        kind: "incident_report",
        media_type: "text/markdown; charset=utf-8",
    },
];

const INCIDENT_REPRODUCTION: CriterionSpec = CriterionSpec::required_deterministic(
    "incident_reproduction",
    15,
    "The seeded timeout and redelivery deterministically reproduce two settlements for one event before remediation.",
    EvaluationDimension::Deliverable,
);
const EVIDENCE_GROUNDED_DIAGNOSIS: CriterionSpec = CriterionSpec::required_deterministic(
    "evidence_grounded_diagnosis",
    20,
    "Independent read-only analyses fan in to a diagnosis grounded in valid evidence and an executed falsification probe.",
    EvaluationDimension::StructuralIntegrity,
);
const REMEDIATION_INTEGRITY: CriterionSpec = CriterionSpec::required_deterministic(
    "remediation_integrity",
    25,
    "Any candidate changes only allowed production paths, preserves protected inputs, and passes every deterministic safety probe.",
    EvaluationDimension::Deliverable,
);
const SAFE_TERMINAL_ACTION: CriterionSpec = CriterionSpec::required_deterministic(
    "safe_terminal_action",
    25,
    "Exactly one terminal action occurs: promote the exact validated candidate or restore the exact known-good revision.",
    EvaluationDimension::StructuralIntegrity,
);
const FINAL_RECONCILIATION: CriterionSpec = CriterionSpec::required_deterministic(
    "final_reconciliation",
    15,
    "Deploy, ledger, audit, incident, active-resource, evidence, and cleanup state reconcile to the selected terminal action.",
    EvaluationDimension::StructuralIntegrity,
);

pub const CRITERIA: [CriterionSpec; 5] = [
    INCIDENT_REPRODUCTION,
    EVIDENCE_GROUNDED_DIAGNOSIS,
    REMEDIATION_INTEGRITY,
    SAFE_TERMINAL_ACTION,
    FINAL_RECONCILIATION,
];

pub fn scenario(_run_id: &str) -> ScenarioSpec {
    ScenarioSpec {
        id: ID,
        version: VERSION,
        // Composite scenarios retain a purpose prompt for the common scenario
        // contract. The workflow sends bounded node-specific prompts.
        prompt: "Investigate, reproduce, diagnose, remediate, validate, and safely resolve an isolated synthetic software incident in an environment-prepared disposable repository. Preserve deterministic evidence, choose exactly one safe terminal action, and leave fixture restoration to mandatory cleanup.".into(),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 1,
            max_output_tokens: None,
            max_total_tokens: 900_000,
            stuck_timeout_seconds: 600,
        },
        denied_functions: &[],
        criteria: CRITERIA.to_vec(),
        judge_reference: None,
        setup: None,
        evaluate: composite_only_evaluator,
        cleanup: None,
    }
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let spec = scenario(namespace);
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        materialized_inputs()?,
        complexity_profile(),
        vec![
            "e2e::control-plane-v1".to_string(),
            "harness::independent_session".to_string(),
            "iii::functions".to_string(),
            "iii::coder".to_string(),
            "iii::shell".to_string(),
            "incident_fixture::v1".to_string(),
        ],
        // Composite assets belong to semantic workflow steps and are captured
        // by the scheduler. Ordinary scenario captures are intentionally empty.
        DeliverableContract::default(),
    )?;
    Ok(MaterializedScenario {
        spec,
        case,
        capture: None,
    })
}

pub fn complexity_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 6,
        dependency_depth: 8,
        parallel_branches: 3,
        external_systems: 4,
        state_transitions: 14,
        wake_cycles: 0,
        validation_loops: 2,
        artifact_count: ASSETS.len() as u8,
        coordination_edges: 16,
        ambiguity_level: 7,
    }
}

pub fn expected_fixture_contract_identity() -> Value {
    json!({
        "schema_version": 1,
        "repository": "iii-hq/incident-response-e2e-fixture",
        "revision_refs": {
            "known_good": KNOWN_GOOD_REF,
            "incident": INCIDENT_REF,
        },
        "allowed_path_patterns": ALLOWED_PATH_PATTERNS,
        "protected_path_patterns": PROTECTED_PATH_PATTERNS,
        "public_probe_ids": PUBLIC_PROBE_IDS,
        "incident_event_id": INCIDENT_EVENT_ID,
        "expected_invariants": EXPECTED_INVARIANTS,
    })
}

fn materialized_inputs() -> anyhow::Result<Value> {
    let fixture_contract_sha256 =
        crate::artifact::sha256_value(&expected_fixture_contract_identity())?;
    let hidden_probe_manifest_sha256 = crate::artifact::sha256_value(&json!({
        "contract": "incident-hidden-probes-v1",
        "probe_count": 5,
    }))?;
    Ok(json!({
        "variant": "duplicate_payment_settlement_after_ack_timeout",
        "fixture_source": FIXTURE_PATH_ENV,
        "fixture_contract_sha256": fixture_contract_sha256,
        "revision_identities": {
            "known_good": {
                "ref": KNOWN_GOOD_REF,
                "resolution": "full_git_sha_at_preflight",
            },
            "incident": {
                "ref": INCIDENT_REF,
                "resolution": "full_git_sha_at_preflight",
            },
        },
        "incident_event_id": INCIDENT_EVENT_ID,
        "expected_invariant_ids": EXPECTED_INVARIANTS,
        "allowed_path_patterns": ALLOWED_PATH_PATTERNS,
        "protected_path_patterns": PROTECTED_PATH_PATTERNS,
        "public_probe_ids": PUBLIC_PROBE_IDS,
        "hidden_probe_manifest_sha256": hidden_probe_manifest_sha256,
        "maximum_repair_rounds": MAX_REPAIR_ROUNDS,
        "workflow_resource_budgets": {
            "max_parallel": 3,
            "max_nodes": 20,
            "step_timeout_seconds": 600,
            "workflow_timeout_seconds": 3600,
            "max_total_tokens": 900000,
            "max_cost_usd": 30.0,
            "technical_retries": 0,
        },
    }))
}

fn composite_only_evaluator<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        bail!("incident_response must be executed through the registered CompositeFlow driver")
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::assessment::{AssessmentKind, AssessmentPolicy, AssessmentSource};

    use super::*;

    #[test]
    fn materialized_contract_is_stable_and_l5() {
        let first = materialize("attempt-a", 42).unwrap();
        let retry = materialize("attempt-b", 42).unwrap();
        let rotated = materialize("attempt-c", 43).unwrap();

        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.case.case_id, rotated.case.case_id);
        assert_eq!(first.case.scenario_version, VERSION);
        assert_eq!(
            first.case.complexity.tier,
            crate::scenarios::ComplexityTier::L5Adaptive
        );
        assert_eq!(first.case.complexity.profile, complexity_profile());
        assert_eq!(first.case.complexity.profile.artifact_count, 11);
        assert_eq!(first.case.work.minimum_expected_work, 36);
        assert!(first.case.deliverable_contract.artifacts.is_empty());
        assert!(first.capture.is_none());
    }

    #[test]
    fn case_inputs_are_stable_non_secret_contract_data() {
        let materialized = materialize("attempt", 7).unwrap();
        let inputs = &materialized.case.inputs;

        assert_eq!(inputs["incident_event_id"], INCIDENT_EVENT_ID);
        assert_eq!(
            inputs.pointer("/revision_identities/known_good/ref"),
            Some(&Value::String(KNOWN_GOOD_REF.into()))
        );
        assert_eq!(
            inputs.pointer("/revision_identities/incident/ref"),
            Some(&Value::String(INCIDENT_REF.into()))
        );
        for pointer in ["/fixture_contract_sha256", "/hidden_probe_manifest_sha256"] {
            let digest = inputs.pointer(pointer).and_then(Value::as_str).unwrap();
            assert!(digest.starts_with("sha256:"));
            assert_eq!(digest.len(), 71);
        }
        let encoded = inputs.to_string().to_ascii_lowercase();
        for forbidden in ["password", "authorization", "private_key", "access_token"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn required_capabilities_match_the_fixture_boundary() {
        let materialized = materialize("attempt", 7).unwrap();
        assert_eq!(
            materialized.case.required_capabilities,
            [
                "e2e::control-plane-v1",
                "harness::independent_session",
                "iii::functions",
                "iii::coder",
                "iii::shell",
                "incident_fixture::v1",
            ]
        );
    }

    #[test]
    fn assessment_contract_is_deterministic_hard_gated_and_totals_one_hundred() {
        let spec = scenario("attempt");
        spec.validate().unwrap();

        assert_eq!(
            spec.criteria
                .iter()
                .map(|criterion| u16::from(criterion.weight))
                .sum::<u16>(),
            100
        );
        assert!(spec.criteria.iter().all(|criterion| {
            criterion.kind == AssessmentKind::RequiredCheck
                && criterion.policy == AssessmentPolicy::HardGate
                && criterion.source == AssessmentSource::Deterministic
        }));
        assert_eq!(
            spec.criteria
                .iter()
                .map(|criterion| criterion.id)
                .collect::<Vec<_>>(),
            [
                "incident_reproduction",
                "evidence_grounded_diagnosis",
                "remediation_integrity",
                "safe_terminal_action",
                "final_reconciliation",
            ]
        );
    }

    #[test]
    fn workflow_asset_catalog_is_complete_unique_and_bounded() {
        let ids = ASSETS.iter().map(|asset| asset.id).collect::<HashSet<_>>();
        assert_eq!(ids.len(), ASSETS.len());
        assert_eq!(ASSETS.len(), 11);
        assert!(ASSETS.iter().all(|asset| {
            !asset.id.is_empty()
                && !asset.producer_node.is_empty()
                && !asset.kind.is_empty()
                && !asset.media_type.is_empty()
        }));
        assert_eq!(
            ASSETS
                .iter()
                .find(|asset| asset.id == "remediation_patch")
                .unwrap()
                .media_type,
            "text/x-diff; charset=utf-8"
        );
        assert_eq!(
            ASSETS
                .iter()
                .find(|asset| asset.id == "incident_report")
                .unwrap()
                .producer_node,
            "validate_incident_report"
        );
    }

    #[test]
    fn resource_budgets_match_the_code_owned_workflow_contract() {
        let inputs = materialize("attempt", 7).unwrap().case.inputs;
        let budgets = &inputs["workflow_resource_budgets"];
        assert_eq!(budgets["max_parallel"], 3);
        assert_eq!(budgets["max_nodes"], 20);
        assert_eq!(budgets["step_timeout_seconds"], 600);
        assert_eq!(budgets["workflow_timeout_seconds"], 3_600);
        assert_eq!(budgets["max_total_tokens"], 900_000);
        assert_eq!(budgets["max_cost_usd"], 30.0);
        assert_eq!(budgets["technical_retries"], 0);
    }
}
