//! Contract for the bounded `incident_response` adaptive scenario.
//!
//! The executable graph and the environment-owned fixture functions live in
//! `workflow::incident_response`. This module deliberately owns only the
//! stable scenario identity, materialized case, deterministic assessment
//! declarations, and the domain-asset catalog. Adaptive workflow assets are
//! persisted by the workflow scheduler before its mandatory cleanup, so they
//! are not duplicated as ordinary `ScenarioDeliverableCapture` hooks here.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    async_trait, Capability, CriterionSpec, DeliverableContract, ExecutionPolicy, ExecutionRealism,
    HumanHorizon, ObjectiveEvaluation, Scenario, ScenarioCase, ScenarioCharacterization,
    ScenarioExecutionKind, ScenarioObservation, ScenarioSpec, ShadowMode,
};

pub const ID: &str = "incident_response";
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

const INCIDENT_REPRODUCTION: CriterionSpec = CriterionSpec::scored(
    "incident_reproduction",
    15,
    "The seeded timeout and redelivery deterministically reproduce two settlements for one event before remediation.",
    EvaluationDimension::Deliverable,
);
const EVIDENCE_GROUNDED_DIAGNOSIS: CriterionSpec = CriterionSpec::scored(
    "evidence_grounded_diagnosis",
    20,
    "Independent read-only analyses fan in to a diagnosis grounded in valid evidence and an executed falsification probe.",
    EvaluationDimension::StructuralIntegrity,
);
const REMEDIATION_INTEGRITY: CriterionSpec = CriterionSpec::scored(
    "remediation_integrity",
    25,
    "Any candidate changes only allowed production paths, preserves protected inputs, and passes every deterministic safety probe.",
    EvaluationDimension::Deliverable,
);
const SAFE_TERMINAL_ACTION: CriterionSpec = CriterionSpec::scored(
    "safe_terminal_action",
    25,
    "Exactly one terminal action occurs: promote the exact validated candidate or restore the exact known-good revision.",
    EvaluationDimension::StructuralIntegrity,
);
const FINAL_RECONCILIATION: CriterionSpec = CriterionSpec::scored(
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

pub struct IncidentResponse;

#[async_trait]
impl Scenario for IncidentResponse {
    fn id(&self) -> &'static str {
        ID
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::AdaptiveFlow
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        ScenarioCharacterization::new(
            HumanHorizon::author_estimate(60, 120)?,
            ExecutionRealism::RealisticSimulator,
            ShadowMode::None,
        )
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            materialized_inputs()?,
            vec![
                Capability::E2eControlPlaneV1,
                Capability::HarnessIndependentSession,
                Capability::IiiFunctions,
                Capability::IiiCoder,
                Capability::IiiShell,
                Capability::IncidentFixtureV1,
            ],
            // Adaptive assets belong to semantic workflow steps and are captured
            // by the scheduler. Ordinary scenario captures are intentionally empty.
            DeliverableContract::default(),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            // Adaptive scenarios retain a purpose prompt for the common scenario
            // contract. The workflow sends bounded node-specific prompts.
            prompt: "Investigate, reproduce, diagnose, remediate, validate, and safely resolve an isolated synthetic software incident in an environment-prepared disposable repository. Preserve deterministic evidence, choose exactly one safe terminal action, and leave fixture restoration to mandatory cleanup.".into(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(1),
                max_output_tokens: None,
                max_total_tokens: Some(750_000),
                stuck_timeout_seconds: Some(600),
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: CRITERIA.to_vec(),
        }
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        _observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        bail!("incident_response must be executed through the registered AdaptiveFlow driver")
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
        "contract": "incident-hidden-probes",
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
            "max_total_tokens": 686000,
            "planner_max_total_tokens": 64000,
            "max_cost_usd": 25.0,
            "technical_retries": 0,
        },
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::assessment::{AssessmentKind, AssessmentPolicy};
    use crate::scenarios::ScenarioId;

    use super::*;

    #[test]
    fn materialized_contract_is_stable() {
        let first = ScenarioId::IncidentResponse
            .materialize("attempt-a", 42)
            .unwrap();
        let retry = ScenarioId::IncidentResponse
            .materialize("attempt-b", 42)
            .unwrap();
        let rotated = ScenarioId::IncidentResponse
            .materialize("attempt-c", 43)
            .unwrap();

        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.case.case_id, rotated.case.case_id);
        assert!(first.case.deliverable_contract.artifacts.is_empty());
    }

    #[test]
    fn case_inputs_are_stable_non_secret_contract_data() {
        let materialized = ScenarioId::IncidentResponse
            .materialize("attempt", 7)
            .unwrap();
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
        let materialized = ScenarioId::IncidentResponse
            .materialize("attempt", 7)
            .unwrap();
        assert_eq!(
            materialized.case.required_capabilities,
            [
                Capability::E2eControlPlaneV1,
                Capability::HarnessIndependentSession,
                Capability::IiiFunctions,
                Capability::IiiCoder,
                Capability::IiiShell,
                Capability::IncidentFixtureV1,
            ]
        );
    }

    #[test]
    fn assessment_contract_is_numeric_and_totals_one_hundred() {
        let spec = IncidentResponse.spec("attempt");
        spec.validate().unwrap();

        assert_eq!(
            spec.criteria
                .iter()
                .map(|criterion| u16::from(criterion.weight))
                .sum::<u16>(),
            100
        );
        assert!(spec.criteria.iter().all(|criterion| {
            criterion.kind == AssessmentKind::Signal
                && criterion.policy == AssessmentPolicy::Advisory
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
        let inputs = ScenarioId::IncidentResponse
            .materialize("attempt", 7)
            .unwrap()
            .case
            .inputs;
        let budgets = &inputs["workflow_resource_budgets"];
        assert_eq!(budgets["max_parallel"], 3);
        assert_eq!(budgets["max_nodes"], 20);
        assert_eq!(budgets["step_timeout_seconds"], 600);
        assert_eq!(budgets["workflow_timeout_seconds"], 3_600);
        assert_eq!(budgets["max_total_tokens"], 686_000);
        assert_eq!(budgets["planner_max_total_tokens"], 64_000);
        assert_eq!(budgets["max_cost_usd"], 25.0);
        assert_eq!(budgets["technical_retries"], 0);
    }
}
