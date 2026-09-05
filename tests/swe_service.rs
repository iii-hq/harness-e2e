use harness_e2e::scenarios::{ScenarioExecutionKind, ScenarioId};
use harness_e2e::workflow::{composite_definition, composite_descriptor_catalog};

const SWE_CASES: [&str; 9] = [
    "swe_config_isolation",
    "swe_cache_invalidation",
    "swe_batch_replay",
    "swe_replay_recovery",
    "swe_contract_migration",
    "swe_tenant_isolation",
    "swe_replay_performance",
    "swe_release_handoff",
    "swe_service_journey",
];

#[test]
fn every_swe_case_is_discoverable_and_materializes_a_non_retryable_workflow() {
    for name in SWE_CASES {
        let parsed = serde_json::from_value::<ScenarioId>(serde_json::json!(name));
        assert!(
            parsed.is_ok(),
            "SWE case {name} is missing from the public catalog"
        );
        let scenario = parsed.unwrap();
        assert!(ScenarioId::ALL.contains(&scenario));
        assert_eq!(scenario.as_str(), name);
        assert_eq!(
            scenario.execution_kind(),
            ScenarioExecutionKind::CompositeFlow
        );
        assert!(!scenario.execution_kind().replay_safe());
        let materialized = scenario.materialize("swe-contract-test", 17).unwrap();
        materialized.validate().unwrap();
        let definition = composite_definition(scenario).unwrap();
        let descriptors = composite_descriptor_catalog(&[scenario]).unwrap();
        definition.validate(&descriptors).unwrap();
        assert_eq!(definition.limits.technical_retries, 0);
        assert_eq!(
            definition.limits.workflow_timeout_seconds,
            if name == "swe_service_journey" {
                5400
            } else {
                900
            }
        );
    }
}

#[test]
fn isolated_cases_and_journey_have_distinct_reproducible_case_identities() {
    let mut identities = std::collections::BTreeSet::new();
    for name in SWE_CASES {
        let scenario: ScenarioId = serde_json::from_value(serde_json::json!(name))
            .unwrap_or_else(|error| panic!("missing {name}: {error}"));
        let first = scenario.materialize("first-attempt", 11).unwrap();
        let second = scenario.materialize("second-attempt", 99).unwrap();
        assert_eq!(first.case.case_id, second.case.case_id);
        assert_eq!(first.case.inputs_sha256, second.case.inputs_sha256);
        assert!(identities.insert(first.case.case_id));
    }
}
