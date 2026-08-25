use super::*;

fn valid_plan(contract: &TodoTaskContract) -> TodoValidationPlan {
    TodoValidationPlan {
        scenario_version: VERSION,
        task_contract_sha256: contract.contract_sha256.clone(),
        summary: "Build the fixed Todo surface and validate it independently.".into(),
        implementation_tasks: vec![TodoImplementationTask {
            id: "implement_worker".into(),
            objective: "Implement the four Todo functions and manifest.".into(),
            completion_signal: "The run-scoped worker is live.".into(),
        }],
        validation_checks: REQUIRED_PROBES
            .iter()
            .map(|probe| TodoValidationCheck {
                id: format!("gate_{probe}"),
                probe_id: (*probe).into(),
                rationale: format!("Validate {probe} deterministically."),
                repetitions: None,
                concurrency: None,
            })
            .collect(),
    }
}

fn passing_bundle(contract: &TodoTaskContract) -> ValidationEvidenceBundle {
    let candidate = format!("sha256:{}", "d".repeat(64));
    ValidationEvidenceBundle {
        scenario_version: VERSION,
        contract_sha256: contract.contract_sha256.clone(),
        plan_sha256: None,
        validator: ValidatorIdentity {
            id: "todo-probe-runner".into(),
            contract_sha256: contract.contract_sha256.clone(),
        },
        subject: ValidatedSubject {
            worker_name: contract.worker_name.clone(),
            candidate_sha256: Some(candidate.clone()),
            source_sha256: Some(candidate.clone()),
            manifest_sha256: Some(format!("sha256:{}", "e".repeat(64))),
            accepted_candidate_sha256: Some(candidate),
            accepted_candidate_is_current: true,
            function_schema_sha256: BTreeMap::new(),
        },
        coverage: ValidationCoverage {
            required: REQUIRED_PROBES
                .iter()
                .map(|probe| (*probe).into())
                .collect(),
            covered: REQUIRED_PROBES
                .iter()
                .map(|probe| (*probe).into())
                .collect(),
            omitted: Vec::new(),
            complete: true,
        },
        attempts: vec![ValidationAttempt {
            ordinal: 1,
            candidate_sha256: Some(format!("sha256:{}", "d".repeat(64))),
            verdict: "passed".into(),
            persisted_before_feedback: true,
            probes: REQUIRED_PROBES
                .iter()
                .map(|probe_id| ProbeObservation {
                    id: (*probe_id).into(),
                    kind: "fixture".into(),
                    expected: json!({"passed": true}),
                    observed: json!({"passed": true}),
                    outcome: ProbeOutcome::Passed,
                    duration_ms: 1,
                    repetition: 1,
                })
                .collect(),
        }],
        nudges: 0,
        repeatability: RepeatabilityEvidence {
            planned: 1,
            completed: 1,
            passed: 1,
        },
        limitations: Vec::new(),
    }
}

#[test]
fn contract_is_run_scoped_and_complete() {
    let contract = task_contract("ABC-123").unwrap();
    assert_eq!(contract.worker_name, "todo-e2e-ABC12300");
    assert!(contract.workspace_root.ends_with(&contract.worker_name));
    contract.validate().unwrap();
    assert_eq!(contract.function_ids.len(), 4);
    assert_eq!(contract.request_response_schemas.len(), 4);
}

#[test]
fn simple_scenario_has_a_bounded_harness_budget() {
    let scenario = simple_scenario("ABC-123");
    assert_eq!(scenario.execution.max_total_tokens, Some(600_000));
}

#[test]
fn evidence_requires_every_mandatory_probe() {
    let contract = task_contract("evidence").unwrap();
    let bundle = ValidationEvidenceBundle {
        scenario_version: VERSION,
        contract_sha256: contract.contract_sha256.clone(),
        plan_sha256: None,
        validator: ValidatorIdentity {
            id: "test".into(),
            contract_sha256: contract.contract_sha256,
        },
        subject: ValidatedSubject {
            worker_name: contract.worker_name,
            candidate_sha256: Some("candidate".into()),
            source_sha256: Some("candidate".into()),
            manifest_sha256: Some("manifest".into()),
            accepted_candidate_sha256: Some("candidate".into()),
            accepted_candidate_is_current: true,
            function_schema_sha256: BTreeMap::new(),
        },
        coverage: ValidationCoverage {
            required: REQUIRED_PROBES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            covered: Vec::new(),
            omitted: REQUIRED_PROBES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            complete: false,
        },
        attempts: vec![ValidationAttempt {
            ordinal: 1,
            candidate_sha256: Some("candidate".into()),
            verdict: "failed".into(),
            persisted_before_feedback: true,
            probes: Vec::new(),
        }],
        nudges: 0,
        repeatability: RepeatabilityEvidence {
            planned: 1,
            completed: 0,
            passed: 0,
        },
        limitations: Vec::new(),
    };
    assert!(!bundle.evidence_complete());
    assert!(!bundle.probe_passed("manifest_valid"));
}

#[test]
fn cleanup_refuses_mismatched_owner_marker() {
    let worker = format!("todo-e2e-cleanup-{}", std::process::id());
    let contract = contract_for_identity(&worker, &workspace_base().join(&worker)).unwrap();
    prepare_owned_workspace(&contract).unwrap();
    fs::write(
        Path::new(&contract.workspace_root).join(OWNER_MARKER),
        "someone-else",
    )
    .unwrap();
    let error = remove_owned_workspace(&contract).unwrap_err().to_string();
    assert!(error.contains("mismatched ownership"));
    let _ = fs::remove_dir_all(&contract.workspace_root);
}

#[test]
fn valid_plan_compiles_deterministically_and_is_hash_bound() {
    let contract = task_contract("compile-valid").unwrap();
    let raw = serde_json::to_vec(&valid_plan(&contract)).unwrap();
    let first = compile_validation_plan(Some(&raw), &contract).unwrap();
    let second = compile_validation_plan(Some(&raw), &contract).unwrap();
    assert_eq!(first, second);
    assert!(first.ready_for_build);
    assert!(first.diagnostics.is_empty());
    assert_eq!(first.compiled_checks.len(), REQUIRED_PROBES.len());
    first.validate_integrity().unwrap();

    let mut tampered = first;
    tampered.implementation_tasks[0].objective = "tampered".into();
    assert!(tampered.validate_integrity().is_err());
}

#[test]
fn incomplete_plan_fails_coverage_without_becoming_a_compiler_error() {
    let contract = task_contract("compile-incomplete").unwrap();
    let mut plan = valid_plan(&contract);
    plan.validation_checks
        .retain(|check| check.probe_id != "todo_invalid_contracts");
    let raw = serde_json::to_vec(&plan).unwrap();
    let compiled = compile_validation_plan(Some(&raw), &contract).unwrap();
    assert!(!compiled.ready_for_build);
    assert!(compiled.planning_gate("plan_present"));
    assert!(compiled.planning_gate("plan_schema_valid"));
    assert!(compiled.planning_gate("plan_compilable"));
    assert!(!compiled.planning_gate("plan_coverage_complete"));
    assert!(compiled
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "coverage_incomplete"));
}

#[test]
fn arbitrary_probe_is_rejected_and_never_compiled() {
    let contract = task_contract("compile-closed-catalog").unwrap();
    let mut plan = valid_plan(&contract);
    plan.validation_checks.push(TodoValidationCheck {
        id: "arbitrary_shell".into(),
        probe_id: "shell::exec rm -rf /".into(),
        rationale: "This must not become executable.".into(),
        repetitions: None,
        concurrency: None,
    });
    let raw = serde_json::to_vec(&plan).unwrap();
    let compiled = compile_validation_plan(Some(&raw), &contract).unwrap();
    assert!(!compiled.ready_for_build);
    assert!(!compiled.planning_gate("plan_compilable"));
    assert!(!compiled
        .compiled_checks
        .iter()
        .any(|check| check.id == "arbitrary_shell"));
}

#[test]
fn final_candidate_pass_is_not_invalidated_by_an_earlier_failed_attempt() {
    let failed = ProbeObservation {
        id: "manifest_valid".into(),
        kind: "manifest".into(),
        expected: json!({"valid": true}),
        observed: json!({"valid": false}),
        outcome: ProbeOutcome::Failed,
        duration_ms: 1,
        repetition: 1,
    };
    let passed = ProbeObservation {
        outcome: ProbeOutcome::Passed,
        observed: json!({"valid": true}),
        ..failed.clone()
    };
    let contract = task_contract("final-candidate").unwrap();
    let bundle = ValidationEvidenceBundle {
        scenario_version: VERSION,
        contract_sha256: contract.contract_sha256.clone(),
        plan_sha256: None,
        validator: ValidatorIdentity {
            id: "test".into(),
            contract_sha256: contract.contract_sha256,
        },
        subject: ValidatedSubject {
            worker_name: contract.worker_name,
            candidate_sha256: Some(format!("sha256:{}", "a".repeat(64))),
            source_sha256: Some(format!("sha256:{}", "a".repeat(64))),
            manifest_sha256: None,
            accepted_candidate_sha256: None,
            accepted_candidate_is_current: false,
            function_schema_sha256: BTreeMap::new(),
        },
        coverage: ValidationCoverage {
            required: vec!["manifest_valid".into()],
            covered: vec!["manifest_valid".into()],
            omitted: Vec::new(),
            complete: true,
        },
        attempts: vec![
            ValidationAttempt {
                ordinal: 1,
                candidate_sha256: None,
                verdict: "failed".into(),
                persisted_before_feedback: true,
                probes: vec![failed],
            },
            ValidationAttempt {
                ordinal: 2,
                candidate_sha256: None,
                verdict: "passed".into(),
                persisted_before_feedback: true,
                probes: vec![passed],
            },
        ],
        nudges: 0,
        repeatability: RepeatabilityEvidence {
            planned: 1,
            completed: 1,
            passed: 1,
        },
        limitations: Vec::new(),
    };
    assert!(bundle.probe_passed("manifest_valid"));
}

#[test]
fn remote_product_rejections_are_distinct_from_transport_failures() {
    let product = anyhow::Error::new(iii_sdk::errors::Error::Remote {
        code: "TODO_NOT_FOUND".into(),
        message: "unknown Todo id".into(),
        stacktrace: None,
    })
    .context("invoke todo::update");
    let transport =
        anyhow::Error::new(iii_sdk::errors::Error::Timeout).context("invoke todo::update");
    assert!(is_remote_invocation_failure(&product));
    assert!(!is_remote_invocation_failure(&transport));
}

#[test]
fn cleanup_retries_only_the_known_worker_lock() {
    assert!(retryable_worker_lock(&anyhow::anyhow!(
        "worker::remove: W900 project lock busy: another worker operation is active"
    )));
    assert!(!retryable_worker_lock(&anyhow::anyhow!(
        "worker::remove: W900 invalid project manifest"
    )));
    assert!(!retryable_worker_lock(&anyhow::anyhow!(
        "worker::remove: connection closed"
    )));
}

#[test]
fn worker_runtime_preconditions_are_infrastructure_not_product_failures() {
    let status = json!({
        "installed": true,
        "running": false,
        "stderr_tail": ["VM execution failed: KVM not accessible -- /dev/kvm permission denied"]
    });
    assert!(worker_mechanism_unavailable_in_status(&status));
    assert!(worker_mechanism_unavailable_in_error(&anyhow::anyhow!(
        "W900 project lock busy: worker operation is active"
    )));
    assert!(!worker_mechanism_unavailable_in_status(&json!({
        "installed": true,
        "running": false,
        "stderr_tail": ["application exited with code 1"]
    })));
}

#[test]
fn known_green_bundle_passes_and_each_broken_probe_turns_its_gate_red() {
    let contract = task_contract("gate-fixtures").unwrap();
    let green = validation_deliverable(&contract, passing_bundle(&contract), SIMPLE_ASSESSMENTS);
    assert!(green.invariants.iter().all(|invariant| invariant.passed));

    for probe_id in REQUIRED_PROBES {
        let mut broken = passing_bundle(&contract);
        broken.attempts[0]
            .probes
            .iter_mut()
            .find(|probe| probe.id == probe_id)
            .unwrap()
            .outcome = ProbeOutcome::Failed;
        let captured = validation_deliverable(&contract, broken, SIMPLE_ASSESSMENTS);
        assert!(
            !captured
                .invariants
                .iter()
                .find(|invariant| invariant.id == probe_id)
                .unwrap()
                .passed,
            "{probe_id} should be red"
        );
    }
}
