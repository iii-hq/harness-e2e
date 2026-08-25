use std::path::PathBuf;

use anyhow::Result;
use harness_e2e::workflow::cross_repo_contract_migration as cross_repo;
use harness_e2e::workflow::release_train_recovery as release;

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(relative)
}

#[test]
fn release_recovery_requires_replan_and_converges_without_bypass() -> Result<()> {
    let mut simulator = release::ReleaseTrainSimulator::from_fixture_path(&fixture_path(
        "release_train_recovery/initial_state.json",
    ))?;

    assert!(simulator
        .apply(release::ReleaseAction::Retag {
            digest: "0".repeat(64)
        })
        .is_err());
    assert!(simulator
        .apply(release::ReleaseAction::BumpVersion {
            version: "0.21.9".into()
        })
        .is_err());
    assert!(simulator
        .apply(release::ReleaseAction::DirectLatestMutation {
            version: "0.21.8".into()
        })
        .is_err());

    simulator.apply(release::ReleaseAction::RerunSameImmutableRun {
        run_id: 424242,
        tag: "workers/v0.21.8".into(),
        version: "0.21.8".into(),
    })?;
    simulator.apply(release::ReleaseAction::PreviewPromotion)?;
    assert!(simulator
        .apply(release::ReleaseAction::RetryStaleOperation {
            operation_id: "promotion-stale-001".into()
        })
        .is_err());
    simulator.apply(release::ReleaseAction::RejectStaleNullCas {
        operation_id: "promotion-stale-001".into(),
    })?;
    assert!(simulator
        .apply(release::ReleaseAction::CreateFreshGatedOperation {
            expected_latest: "wrong-pointer".into()
        })
        .is_err());
    simulator.apply(release::ReleaseAction::CreateFreshGatedOperation {
        expected_latest: "0.20.4".into(),
    })?;
    simulator.apply(release::ReleaseAction::ObserveCanary)?;
    simulator.apply(release::ReleaseAction::ObserveCanary)?;

    let gates = simulator.evaluate();
    assert!(gates.passed(), "release gates did not pass: {gates:?}");
    assert_eq!(simulator.state.latest_cas_count, 1);
    assert_eq!(simulator.audit().len(), 6);
    Ok(())
}

#[test]
fn release_plans_are_bounded_and_second_revision_is_evidence_bound() -> Result<()> {
    let first = release::materialize_plan(&release::PlanRevisionRequest {
        revision: 1,
        selected_templates: vec![
            "inspect_partial_publication".into(),
            "inspect_registry_pointer".into(),
        ],
        supersedes_sha256: None,
        evidence_ids: Vec::new(),
    })?;
    assert_eq!(
        first.nodes.first().unwrap().template_id,
        "preflight_release_identity"
    );
    assert_eq!(first.nodes.last().unwrap().template_id, "preview_promotion");

    assert!(release::materialize_plan(&release::PlanRevisionRequest {
        revision: 2,
        selected_templates: vec!["inspect_incompatible_graph".into()],
        supersedes_sha256: Some(first.sha256.clone()),
        evidence_ids: Vec::new(),
    })
    .is_err());
    let second = release::materialize_plan(&release::PlanRevisionRequest {
        revision: 2,
        selected_templates: vec![
            "inspect_incompatible_graph".into(),
            "inspect_operation_history".into(),
        ],
        supersedes_sha256: Some(first.sha256.clone()),
        evidence_ids: vec![release::INVALIDATION_EVIDENCE_ID.into()],
    })?;
    assert_eq!(
        second.supersedes_sha256.as_deref(),
        Some(first.sha256.as_str())
    );
    assert_eq!(
        second.nodes.last().unwrap().template_id,
        "reconcile_release_state"
    );
    assert!(release::template_catalog()
        .iter()
        .all(|template| !template.network_write_allowed));
    Ok(())
}

#[test]
fn release_shadow_is_optional_and_read_only() -> Result<()> {
    let absent = release::load_shadow_evidence(None)?;
    assert_eq!(absent.outcome, release::ShadowOutcome::NotEvaluated);
    assert!(absent.snapshot.is_none());

    let present = release::load_shadow_evidence(Some(&fixture_path(
        "release_train_recovery/shadow_example.json",
    )))?;
    assert_eq!(present.outcome, release::ShadowOutcome::Advisory);
    assert_eq!(present.snapshot.unwrap().github_run_attempt, 2);
    assert_eq!(present.content_sha256.unwrap().len(), 64);
    Ok(())
}

#[test]
fn cross_repo_canary_reveals_hidden_consumer_and_forces_revision_two() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let workspace = temporary.path().join("workspace");
    let fixture = fixture_path("cross_repo_contract_migration");
    let mut simulator = cross_repo::CrossRepoSimulator::materialize(&fixture, &workspace)?;

    assert_eq!(simulator.workspace_root(), workspace);
    assert_eq!(simulator.visible_repositories(), ["producer", "consumer-a"]);
    assert!(!workspace.join("consumer-b").exists());
    assert!(simulator
        .validate_visible_matrix()?
        .iter()
        .all(|result| result.passed));

    simulator.apply_reference_plan_a()?;
    assert!(simulator
        .validate_visible_matrix()?
        .iter()
        .all(|result| result.passed));
    let canary = simulator.run_trusted_canary()?;
    assert!(!canary.passed);
    assert_eq!(canary.evidence_id, cross_repo::CANARY_EVIDENCE_ID);
    assert!(workspace.join("consumer-b/.git").is_dir());

    simulator.apply_reference_plan_b()?;
    let matrix = simulator.validate_full_matrix()?;
    assert_eq!(matrix.len(), 2);
    assert!(
        matrix.iter().all(|result| result.passed),
        "matrix: {matrix:?}"
    );
    let boundaries = simulator.validate_boundaries()?;
    assert!(
        boundaries.passed(),
        "workspace gates did not pass: {boundaries:?}"
    );
    assert!(simulator
        .reject_network_access("https://example.com")
        .is_err());
    simulator.cleanup()?;
    assert!(simulator.cleanup_complete());
    Ok(())
}

#[test]
fn cross_repo_materialization_produces_deterministic_initial_commits() -> Result<()> {
    let fixture = fixture_path("cross_repo_contract_migration");
    let first_temp = tempfile::tempdir()?;
    let second_temp = tempfile::tempdir()?;
    let first =
        cross_repo::CrossRepoSimulator::materialize(&fixture, &first_temp.path().join("one"))?;
    let second =
        cross_repo::CrossRepoSimulator::materialize(&fixture, &second_temp.path().join("two"))?;
    assert_eq!(
        first.initial_commit("producer"),
        second.initial_commit("producer")
    );
    assert_eq!(
        first.initial_commit("consumer-a"),
        second.initial_commit("consumer-a")
    );
    Ok(())
}

#[test]
fn cross_repo_second_plan_requires_canary_evidence() -> Result<()> {
    let first = cross_repo::materialize_plan(&cross_repo::PlanRevisionRequest {
        revision: 1,
        selected_templates: vec![
            "inspect_producer_contract".into(),
            "inspect_consumer_a".into(),
        ],
        supersedes_sha256: None,
        evidence_ids: Vec::new(),
    })?;
    assert_eq!(
        first.nodes.last().unwrap().template_id,
        "reveal_consumer_b_canary"
    );

    assert!(
        cross_repo::materialize_plan(&cross_repo::PlanRevisionRequest {
            revision: 2,
            selected_templates: vec!["inspect_consumer_b".into()],
            supersedes_sha256: Some(first.sha256.clone()),
            evidence_ids: vec!["unrelated".into()],
        })
        .is_err()
    );
    let second = cross_repo::materialize_plan(&cross_repo::PlanRevisionRequest {
        revision: 2,
        selected_templates: vec![
            "inspect_consumer_b".into(),
            "inspect_legacy_alias_history".into(),
        ],
        supersedes_sha256: Some(first.sha256),
        evidence_ids: vec![cross_repo::CANARY_EVIDENCE_ID.into()],
    })?;
    assert_eq!(
        second.nodes.last().unwrap().template_id,
        "cleanup_workspace"
    );
    assert!(cross_repo::template_catalog()
        .iter()
        .all(|template| !template.network_allowed));
    Ok(())
}
