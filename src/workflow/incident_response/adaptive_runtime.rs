use std::collections::BTreeSet;

use anyhow::Result;

use super::definition;
use crate::workflow::{
    AdaptiveAnchorPlacement, AdaptiveNodeTemplateV1, AdaptivePlanNodeV1, AdaptiveTrustedAnchorV1,
    AdaptiveWorkflowPlanV1, AdaptiveWorkflowPolicyV1, WorkflowNodeV1,
    ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
};

pub const INVALIDATION_EVIDENCE_ID: &str = "incident_candidate_validation/v1";

#[derive(Debug, Clone)]
pub struct IncidentAdaptiveContract {
    pub policy: AdaptiveWorkflowPolicyV1,
    pub plans: Vec<AdaptiveWorkflowPlanV1>,
    pub completed_node_ids: BTreeSet<String>,
}

pub fn adaptive_contract() -> Result<IncidentAdaptiveContract> {
    let source = definition();
    let before_ids = [
        "preflight_fixture",
        "capture_baseline",
        "deduplicate_alert",
        "reproduce_incident",
    ];
    let after_ids = [
        "decide_terminal_action",
        "promote_candidate",
        "rollback_candidate",
        "reconcile_final_state",
        "write_incident_report",
        "validate_incident_report",
    ];
    let templates = source
        .nodes
        .iter()
        .filter(|node| !before_ids.contains(&node.id.as_str()))
        .filter(|node| !after_ids.contains(&node.id.as_str()))
        .map(template_from_node)
        .collect::<Vec<_>>();
    let trusted_anchors = source
        .nodes
        .iter()
        .filter_map(|node| {
            if before_ids.contains(&node.id.as_str()) {
                Some(AdaptiveTrustedAnchorV1 {
                    placement: AdaptiveAnchorPlacement::BeforePlan,
                    terminal: false,
                    node: node.clone(),
                })
            } else if after_ids.contains(&node.id.as_str()) {
                Some(AdaptiveTrustedAnchorV1 {
                    placement: AdaptiveAnchorPlacement::AfterPlan,
                    terminal: node.id == "validate_incident_report",
                    node: node.clone(),
                })
            } else {
                None
            }
        })
        .collect();
    let policy = AdaptiveWorkflowPolicyV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        id: source.id,
        scenario_version: source.scenario_version,
        description: "Runner-bounded incident response with agent-owned investigation and remediation decomposition, one evidence-bound revision, and trusted mutation/reconciliation anchors.".into(),
        limits: source.limits,
        max_plan_nodes: 8,
        max_plan_depth: 8,
        max_plan_revisions: 2,
        max_instruction_bytes: 8 * 1024,
        templates,
        trusted_anchors,
        criteria: source.criteria,
    };
    let policy_sha256 = policy.canonical_sha256()?;
    let revision_one = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256: policy_sha256.clone(),
        revision: 1,
        supersedes_sha256: None,
        reason: None,
        evidence_ids: Vec::new(),
        nodes: planned_nodes(false),
    };
    let revision_one_sha256 = revision_one.canonical_sha256()?;
    let revision_two = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256,
        revision: 2,
        supersedes_sha256: Some(revision_one_sha256),
        reason: Some(
            "Trusted candidate validation requires a bounded remediation and validation branch before terminal action"
                .into(),
        ),
        evidence_ids: vec![INVALIDATION_EVIDENCE_ID.into()],
        nodes: planned_nodes(true),
    };
    Ok(IncidentAdaptiveContract {
        policy,
        plans: vec![revision_one, revision_two],
        completed_node_ids: BTreeSet::new(),
    })
}

fn template_from_node(node: &WorkflowNodeV1) -> AdaptiveNodeTemplateV1 {
    AdaptiveNodeTemplateV1 {
        id: node.id.clone(),
        description: format!("Allowlisted incident-response capability for {}", node.id),
        step_type: node.step_type.clone(),
        step_version: node.step_version,
        base_config: node.config.clone(),
        inputs: node.inputs.clone(),
        activation: node.activation.clone(),
        dependency_policy: node.dependency_policy,
        required: node.required,
        allowed_focuses: Vec::new(),
        focus_config_key: None,
        instructions_config_key: None,
        min_occurrences: 0,
        max_occurrences: 1,
    }
}

fn planned_nodes(include_remediation: bool) -> Vec<AdaptivePlanNodeV1> {
    let source = definition();
    let mut included = vec![
        "analyze_logs",
        "analyze_metrics",
        "analyze_trace_change",
        "validate_triage",
        "synthesize_diagnosis",
        "validate_diagnosis",
    ];
    if include_remediation {
        included.extend(["apply_remediation", "validate_candidate"]);
    }
    included
        .into_iter()
        .map(|id| {
            let node = source
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap_or_else(|| panic!("missing incident workflow node '{id}'"));
            AdaptivePlanNodeV1 {
                id: node.id.clone(),
                template_id: node.id.clone(),
                depends_on: node.depends_on.clone(),
                focus: None,
                instructions: None,
            }
        })
        .collect()
}
