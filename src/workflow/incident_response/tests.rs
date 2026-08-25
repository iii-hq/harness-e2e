use super::evaluation::{candidate_gate_vector, decide};
use super::schemas::{ProbeResult, ValidateResponse};
use super::*;

fn descriptor_catalog() -> StepCatalog {
    let mut catalog = StepCatalog::new();
    catalog
        .register_descriptor(super::super::harness_descriptor_v2().unwrap())
        .unwrap();
    for descriptor in descriptors_only().unwrap() {
        catalog.register_descriptor(descriptor).unwrap();
    }
    catalog
}

fn passing_validation() -> ValidateResponse {
    let probes = [
        "focused_tests",
        "duplicate_delivery",
        "concurrent_duplicate",
        "ack_timeout_replay",
        "distinct_events",
        "ledger_invariant",
        "audit_history",
        "full_regression",
        "canary_budget",
    ]
    .into_iter()
    .map(|id| {
        (
            id.to_string(),
            ProbeResult {
                passed: true,
                summary: "passed".into(),
            },
        )
    })
    .collect();
    ValidateResponse {
        candidate_sha: Some("a".repeat(40)),
        changed_paths: vec!["src/settlement.rs".into()],
        protected_paths_unchanged: true,
        tests_unchanged: true,
        fixture_contract_unchanged: true,
        working_tree_candidate_only: true,
        repair_rounds: 1,
        probes,
        patch: "diff --git a/src/settlement.rs b/src/settlement.rs\n".into(),
        before_after_hashes: BTreeMap::new(),
    }
}

#[test]
fn complete_definition_validates_against_descriptor_catalog() {
    let definition = definition();
    let materialized = definition.validate(&descriptor_catalog()).unwrap();
    assert_eq!(materialized.definition.nodes.len(), 18);
    assert_eq!(materialized.definition.limits.max_parallel, 3);
    assert_eq!(materialized.definition.limits.technical_retries, 0);
}

#[test]
fn adaptive_contract_is_evidence_bound_and_materializes_the_full_recovery_graph() {
    let contract = adaptive_contract().unwrap();
    let materialized = contract
        .policy
        .materialize(
            &contract.plans,
            &contract.completed_node_ids,
            &descriptor_catalog(),
        )
        .unwrap();
    assert_eq!(contract.plans.len(), 2);
    assert_eq!(contract.plans[1].evidence_ids, [INVALIDATION_EVIDENCE_ID]);
    assert_eq!(
        materialized.definition.nodes.len(),
        definition().nodes.len()
    );
    assert_eq!(materialized.revisions.len(), 2);
}

#[test]
fn descriptors_are_code_owned_and_have_no_configurable_function_ids() {
    let descriptors = descriptors_only().unwrap();
    assert_eq!(descriptors.len(), 12);
    for descriptor in descriptors {
        descriptor.validate().unwrap();
        assert!(!descriptor.config_schema.to_string().contains("function_id"));
    }
}

#[test]
fn criteria_are_required_deterministic_and_total_one_hundred() {
    let definition = definition();
    assert_eq!(
        definition
            .criteria
            .iter()
            .map(|criterion| u16::from(criterion.weight))
            .sum::<u16>(),
        100
    );
    assert!(definition.criteria.iter().all(|criterion| {
        !criterion.advisory
            && definition
                .nodes
                .iter()
                .find(|node| node.id == criterion.producer_node_id)
                .is_some_and(|node| node.required)
    }));
}

#[test]
fn terminal_decision_is_always_exclusive() {
    let passing = passing_validation();
    for (ready, validation) in [
        (false, None),
        (true, None),
        (false, Some(&passing)),
        (true, Some(&passing)),
    ] {
        let decision = decide(ready, validation);
        assert_ne!(decision.should_promote, decision.should_rollback);
    }
    assert!(decide(true, Some(&passing)).should_promote);
}

#[test]
fn every_individual_candidate_gate_blocks_promotion() {
    let passing = passing_validation();
    assert!(candidate_gate_vector(&passing)
        .iter()
        .all(|(_, passed)| *passed));

    let mut no_patch = passing.clone();
    no_patch.patch.clear();
    assert!(!decide(true, Some(&no_patch)).should_promote);

    let mut protected_changed = passing.clone();
    protected_changed.protected_paths_unchanged = false;
    assert!(!decide(true, Some(&protected_changed)).should_promote);

    let mut hidden_failed = passing.clone();
    hidden_failed
        .probes
        .get_mut("concurrent_duplicate")
        .unwrap()
        .passed = false;
    assert!(!decide(true, Some(&hidden_failed)).should_promote);
}

#[test]
fn graph_keeps_parallel_triage_and_exclusive_terminal_branches() {
    let definition = definition();
    for id in ["analyze_logs", "analyze_metrics", "analyze_trace_change"] {
        let node = definition.nodes.iter().find(|node| node.id == id).unwrap();
        assert_eq!(node.depends_on, ["reproduce_incident"]);
        assert_eq!(node.step_version, super::super::HARNESS_STEP_VERSION_V2);
    }
    let promote = definition
        .nodes
        .iter()
        .find(|node| node.id == "promote_candidate")
        .unwrap();
    let rollback = definition
        .nodes
        .iter()
        .find(|node| node.id == "rollback_candidate")
        .unwrap();
    assert!(!promote.required && !rollback.required);
    assert!(matches!(promote.activation, ActivationPolicy::All(_)));
    assert!(matches!(rollback.activation, ActivationPolicy::All(_)));
}
