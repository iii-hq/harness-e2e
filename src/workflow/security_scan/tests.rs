use super::*;

#[test]
fn scan_report_rejects_unknown_coverage_absolute_paths_and_patches() {
    let report = json!({
        "summary": "bad",
        "assessments": {
            "vulnerabilities": {"status": "unknown"},
            "dependencies": {"status": "assessed"},
            "secrets": {"status": "assessed"},
            "supply_chain": {"status": "assessed"}
        },
        "findings": [{
            "location": {"path": "/tmp/repo/src/main.rs", "line_start": 0},
            "suggested_patch": "patch"
        }]
    });
    let (gates, _) = evaluate_report(
        &report,
        "scan",
        &BTreeSet::new(),
        Some(Path::new("/tmp/repo")),
    )
    .unwrap();
    assert!(gates.iter().all(|gate| !gate.passed));
}

#[test]
fn descriptor_catalog_contains_no_configurable_function_ids() {
    for descriptor in descriptors_only() {
        let encoded = descriptor.config_schema.to_string();
        assert!(!encoded.contains("function_id"));
        descriptor.validate().unwrap();
    }
}

#[test]
fn unavailable_reconciliation_sources_require_null_counts() {
    let snapshot = json!({
        "harness": {"scope": "exact_commit"},
        "sources": [
            {
                "source": "dependabot",
                "scope": "repository_default_branch",
                "status": "unavailable",
                "record_count": 3
            },
            {
                "source": "code_scanning",
                "scope": "repository_snapshot",
                "status": "complete",
                "record_count": 0
            }
        ],
        "records": []
    });
    let gates = evaluate_reconciliation(&snapshot);
    assert!(
        !gates
            .iter()
            .find(|gate| gate.id == "reconciliation_counts")
            .unwrap()
            .passed
    );
    assert!(reconciliation_infrastructure_failure(&snapshot)
        .unwrap()
        .contains("dependabot=unavailable"));

    let mut valid = snapshot;
    valid["sources"][0]["record_count"] = Value::Null;
    let gates = evaluate_reconciliation(&valid);
    assert!(gates.iter().all(|gate| gate.passed));
    assert!(reconciliation_infrastructure_failure(&valid).is_some());
    valid["sources"][0]["status"] = Value::String("complete".into());
    valid["sources"][0]["record_count"] = Value::from(0);
    assert!(reconciliation_infrastructure_failure(&valid).is_none());
}

#[tokio::test]
async fn suggested_patches_are_checked_in_a_disposable_worktree() {
    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("file.txt"), "old\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "file.txt"],
        vec![
            "-c",
            "user.name=Harness E2E",
            "-c",
            "user.email=harness-e2e@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(fixture.path())
            .status()
            .unwrap()
            .success());
    }
    let report = json!({
        "findings": [{
            "suggested_patch": "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n"
        }]
    });
    let evaluation = evaluate_patch_applicability(&report, fixture.path(), "assess")
        .await
        .unwrap();
    assert_eq!(evaluation.outcome, WorkflowEvaluationOutcome::Passed);
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("file.txt")).unwrap(),
        "old\n"
    );
    assert!(git(fixture.path(), &["status", "--porcelain"])
        .await
        .unwrap()
        .is_empty());
}

#[test]
fn full_workflow_is_valid_against_the_registered_catalog() {
    let mut catalog = StepCatalog::new();
    for descriptor in descriptors_only() {
        catalog.register_descriptor(descriptor).unwrap();
    }
    let definition = definition();
    let materialized = definition.validate(&catalog).unwrap();
    assert_eq!(materialized.definition.scenario_version, 3);
    assert_eq!(materialized.definition.nodes.len(), 5);
    let scan_b = materialized
        .definition
        .nodes
        .iter()
        .find(|node| node.id == "scan_commit_b")
        .expect("commit B must be scanned on demand");
    assert_eq!(scan_b.step_type, "security_review.scan_commit_b");
    assert!(!materialized
        .definition
        .nodes
        .iter()
        .any(|node| node.id.contains("scheduled") || node.step_type.contains("scheduled")));
}
