//! Shared construction for the extended scenario families.

use std::path::PathBuf;

use serde_json::Value;

use super::assessment::{self, AssessmentSpec};
use super::common::ObservedFunctionCall;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, DeliverableContract,
    ExecutionPolicy, InvariantSpec, ProvenanceEvidence, ScenarioCleanup, ScenarioEvaluator,
    ScenarioObservation, ScenarioSetup, ScenarioSpec,
};

pub(super) use super::captured_gate_invariants;

pub(in crate::scenarios) struct Blueprint {
    pub id: &'static str,
    pub version: u32,
    pub prompt: String,
    pub filesystem_root: Option<PathBuf>,
    pub execution: ExecutionPolicy,
    pub assessments: &'static [AssessmentSpec],
    pub setup: Option<ScenarioSetup>,
    pub evaluate: ScenarioEvaluator,
    pub cleanup: Option<ScenarioCleanup>,
}

impl Blueprint {
    pub(in crate::scenarios) fn spec(self) -> ScenarioSpec {
        ScenarioSpec {
            id: self.id,
            version: self.version,
            prompt: self.prompt,
            filesystem_root: self.filesystem_root,
            execution: self.execution,
            denied_functions: &[],
            criteria: assessment::criteria(self.assessments),
            judge_reference: None,
            setup: self.setup,
            evaluate: self.evaluate,
            cleanup: self.cleanup,
        }
    }
}

pub(in crate::scenarios) const fn policy(
    max_turns: u32,
    max_total_tokens: u64,
    stuck_timeout_seconds: u64,
) -> ExecutionPolicy {
    ExecutionPolicy {
        max_turns,
        max_output_tokens: Some(8_192),
        max_total_tokens,
        stuck_timeout_seconds,
    }
}

pub(in crate::scenarios) fn contract(
    artifact_id: &str,
    kind: &str,
    schema: Value,
    assessments: &[AssessmentSpec],
    max_size_bytes: u64,
) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: artifact_id.to_string(),
            kind: kind.to_string(),
            media_type: "application/json".to_string(),
            schema,
            max_size_bytes,
        }],
        invariants: assessments
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().to_string(),
                description: assessment.description().to_string(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

pub(in crate::scenarios) fn evidence(
    id: &str,
    kind: &str,
    content: Value,
    invariants: Vec<CapturedInvariant>,
    provenance: Vec<ProvenanceEvidence>,
) -> CapturedDeliverable {
    CapturedDeliverable {
        id: id.to_string(),
        kind: kind.to_string(),
        content: content.into(),
        invariants,
        provenance,
    }
}

pub(in crate::scenarios) fn session_provenance(
    observation: &ScenarioObservation,
    relation: &str,
) -> ProvenanceEvidence {
    ProvenanceEvidence {
        kind: "session".to_string(),
        source_id: observation.metrics.root_session_id.clone(),
        relation: relation.to_string(),
    }
}

pub(in crate::scenarios) fn function_provenance(
    function_id: &str,
    relation: &str,
) -> ProvenanceEvidence {
    ProvenanceEvidence {
        kind: "function".to_string(),
        source_id: function_id.to_string(),
        relation: relation.to_string(),
    }
}

pub(in crate::scenarios) fn scope(run_id: &str) -> String {
    format!("e2e:{run_id}")
}

pub(in crate::scenarios) fn calls_of<'a>(
    calls: &'a [ObservedFunctionCall],
    function_id: &str,
) -> Vec<&'a ObservedFunctionCall> {
    calls
        .iter()
        .filter(|call| call.function_id == function_id)
        .collect()
}

pub(in crate::scenarios) async fn state_get(
    context: &crate::context::E2eContext,
    scope: &str,
    key: &str,
) -> anyhow::Result<Value> {
    Ok(super::common::state_value(
        context
            .trigger_value(
                "state::get",
                serde_json::json!({ "scope": scope, "key": key }),
            )
            .await?,
    ))
}

pub(in crate::scenarios) async fn state_set(
    context: &crate::context::E2eContext,
    scope: &str,
    key: &str,
    value: Value,
) -> anyhow::Result<()> {
    context
        .trigger_value(
            "state::set",
            serde_json::json!({ "scope": scope, "key": key, "value": value }),
        )
        .await?;
    Ok(())
}

pub(in crate::scenarios) async fn state_delete(
    context: &crate::context::E2eContext,
    scope: &str,
    keys: &[String],
) -> anyhow::Result<()> {
    for key in keys {
        context
            .trigger_value(
                "state::delete",
                serde_json::json!({ "scope": scope, "key": key }),
            )
            .await?;
    }
    Ok(())
}

pub(in crate::scenarios) fn missing_tokens(response: &str, tokens: &[&str]) -> Vec<String> {
    tokens
        .iter()
        .filter(|token| !response.contains(**token))
        .map(|token| (*token).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_only_the_tokens_a_response_is_missing() {
        assert!(missing_tokens("RECORDS:3 MISSING:none", &["RECORDS:3"]).is_empty());
        assert_eq!(
            missing_tokens("RECORDS:3", &["RECORDS:3", "DISCREPANCY:1"]),
            vec!["DISCREPANCY:1".to_string()]
        );
    }

    #[test]
    fn a_contract_mirrors_its_assessments_as_invariants() {
        const ASSESSMENTS: &[AssessmentSpec] =
            &[AssessmentSpec::hard_gated("only_gate", 100, "described")];
        let contract = contract(
            "artifact",
            "kind",
            json!({"type": "object"}),
            ASSESSMENTS,
            1_024,
        );

        assert_eq!(contract.invariants.len(), 1);
        assert_eq!(contract.invariants[0].id, "only_gate");
        assert!(contract.capture_before_cleanup);
    }
}
