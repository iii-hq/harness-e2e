use super::evidence::{bounded_value, outcome, probe};
use super::probes::{
    install_and_wait, is_remote_invocation_failure, run_concurrent_create, run_crud_cycle,
    validate_function_surface, validate_invalid_inputs, validate_manifest,
};
use super::workspace::{candidate_sha256, file_sha256};
use super::*;

#[derive(Debug, Clone)]
pub struct TodoProbeRunner {
    contract: TodoTaskContract,
}

impl TodoProbeRunner {
    pub fn new(contract: TodoTaskContract) -> Result<Self> {
        contract.validate()?;
        Ok(Self { contract })
    }

    pub async fn run(
        &self,
        client: &IIIClient,
        repetitions: u8,
        plan_sha256: Option<String>,
    ) -> Result<ValidationEvidenceBundle> {
        if repetitions == 0 || repetitions > 3 {
            bail!("Todo validation repetitions must be between one and three");
        }
        let candidate_sha256 = candidate_sha256(Path::new(&self.contract.workspace_root))?;
        let manifest_sha256 =
            file_sha256(&Path::new(&self.contract.workspace_root).join("iii.worker.yaml"))?;
        let mut probes = Vec::new();
        let mut limitations = Vec::new();

        let manifest_started = Instant::now();
        let manifest_result = validate_manifest(client, &self.contract).await?;
        let manifest_passed = manifest_result.1 == ProbeOutcome::Passed;
        probes.push(probe(
            "manifest_valid",
            "manifest",
            json!({"valid": true, "name": self.contract.worker_name, "scripts_start": true, "scripts_setup": false}),
            manifest_result.0,
            manifest_result.1,
            manifest_started,
            1,
        ));

        let status_started = Instant::now();
        let (status, worker_live, worker_outcome) = if manifest_passed {
            let (status, running) = install_and_wait(client, &self.contract).await?;
            (status, running, outcome(running))
        } else {
            (
                json!({"skipped": "manifest_valid is a prerequisite"}),
                false,
                ProbeOutcome::NotEvaluated,
            )
        };
        probes.push(probe(
            "worker_live",
            "worker_status",
            json!({"installed": true, "running": true}),
            bounded_value(&status),
            worker_outcome,
            status_started,
            1,
        ));

        let surface_started = Instant::now();
        let (surface_ok, surface_observed, schema_hashes) =
            validate_function_surface(client, &self.contract).await?;
        probes.push(probe(
            "function_surface",
            "function_contract",
            json!({"functions": self.contract.function_ids, "exact_schemas": true, "descriptions": true}),
            surface_observed,
            outcome(surface_ok),
            surface_started,
            1,
        ));

        let mut completed = 0_u8;
        let mut passed = 0_u8;
        if worker_live && surface_ok {
            for repetition in 1..=repetitions {
                completed = completed.saturating_add(1);
                let started = Instant::now();
                let result = run_crud_cycle(client, &self.contract, repetition).await;
                match result {
                    Ok((ok, observed)) => {
                        if ok {
                            passed = passed.saturating_add(1);
                        }
                        probes.push(probe(
                            "todo_crud_isolated",
                            "function_sequence",
                            json!({"operations": ["create", "list", "update", "list", "delete", "list"], "isolated": true}),
                            observed,
                            outcome(ok),
                            started,
                            repetition,
                        ));
                    }
                    Err(error) if !is_remote_invocation_failure(&error) => return Err(error),
                    Err(error) => {
                        probes.push(probe(
                            "todo_crud_isolated",
                            "function_sequence",
                            json!({"operations": ["create", "list", "update", "delete"]}),
                            json!({"error": error.to_string()}),
                            ProbeOutcome::Failed,
                            started,
                            repetition,
                        ));
                    }
                }
            }
        } else {
            probes.push(ProbeObservation {
                id: "todo_crud_isolated".into(),
                kind: "function_sequence".into(),
                expected: json!({"operations": ["create", "list", "update", "delete"]}),
                observed: json!({"skipped": "worker_live and function_surface are prerequisites"}),
                outcome: ProbeOutcome::NotEvaluated,
                duration_ms: 0,
                repetition: 1,
            });
        }

        let invalid_started = Instant::now();
        let invalid = if worker_live && surface_ok {
            validate_invalid_inputs(client, &self.contract).await?
        } else {
            (
                false,
                json!({"skipped": "function surface unavailable"}),
                true,
            )
        };
        probes.push(probe(
            "todo_invalid_contracts",
            "negative_contract",
            json!({"empty_title": "rejected", "unknown_id": "rejected"}),
            invalid.1,
            if invalid.2 {
                ProbeOutcome::NotEvaluated
            } else {
                outcome(invalid.0)
            },
            invalid_started,
            1,
        ));

        if repetitions > 1 {
            probes.push(ProbeObservation {
                id: "todo_repeatability".into(),
                kind: "repeatability".into(),
                expected: json!({"planned": repetitions, "passed": repetitions}),
                observed: json!({"completed": completed, "passed": passed}),
                outcome: outcome(completed == repetitions && passed == repetitions),
                duration_ms: 0,
                repetition: repetitions,
            });
        }

        let covered = probes
            .iter()
            .filter(|probe| probe.outcome != ProbeOutcome::NotEvaluated)
            .map(|probe| probe.id.clone())
            .collect::<BTreeSet<_>>();
        let required = REQUIRED_PROBES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        let omitted = required
            .iter()
            .filter(|id| !covered.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        if candidate_sha256.is_none() {
            limitations.push(
                "Candidate fingerprint is unavailable because no bounded source files were found."
                    .into(),
            );
        }
        let verdict = if probes
            .iter()
            .filter(|probe| REQUIRED_PROBES.contains(&probe.id.as_str()))
            .all(|probe| probe.outcome == ProbeOutcome::Passed)
        {
            "passed"
        } else {
            "failed"
        };
        let contract_sha256 = self.contract.contract_sha256.clone();
        Ok(ValidationEvidenceBundle {
            scenario_version: VERSION,
            contract_sha256: contract_sha256.clone(),
            plan_sha256,
            validator: ValidatorIdentity {
                id: "todo-probe-runner".into(),
                contract_sha256,
            },
            subject: ValidatedSubject {
                worker_name: self.contract.worker_name.clone(),
                candidate_sha256: candidate_sha256.clone(),
                source_sha256: candidate_sha256.clone(),
                manifest_sha256,
                accepted_candidate_sha256: candidate_sha256.clone(),
                accepted_candidate_is_current: candidate_sha256.is_some(),
                function_schema_sha256: schema_hashes,
            },
            coverage: ValidationCoverage {
                required,
                covered: covered.into_iter().collect(),
                omitted: omitted.clone(),
                complete: omitted.is_empty(),
            },
            attempts: vec![ValidationAttempt {
                ordinal: 1,
                candidate_sha256,
                verdict: verdict.into(),
                persisted_before_feedback: true,
                probes,
            }],
            nudges: 0,
            repeatability: RepeatabilityEvidence {
                planned: repetitions,
                completed,
                passed,
            },
            limitations,
        })
    }

    pub async fn run_compiled(
        &self,
        client: &IIIClient,
        plan: &CompiledValidationPlan,
    ) -> Result<ValidationEvidenceBundle> {
        if !plan.ready_for_build
            || plan.task_contract.contract_sha256 != self.contract.contract_sha256
        {
            bail!("Todo compiled plan is not ready or references a different contract");
        }
        let repetitions = plan
            .selected_probe("todo_repeatability")
            .and_then(|check| check.repetitions)
            .unwrap_or(1);
        let mut bundle = self
            .run(client, repetitions, Some(plan.compiled_plan_sha256.clone()))
            .await?;
        if plan.selected_probe("todo_repeatability").is_some()
            && bundle.attempts.last().is_some_and(|attempt| {
                !attempt
                    .probes
                    .iter()
                    .any(|probe| probe.id == "todo_repeatability")
            })
        {
            let passed = bundle.repeatability.completed == repetitions
                && bundle.repeatability.passed == repetitions;
            bundle
                .attempts
                .last_mut()
                .expect("runner always records an attempt")
                .probes
                .push(ProbeObservation {
                    id: "todo_repeatability".into(),
                    kind: "repeatability".into(),
                    expected: json!({"planned": repetitions, "passed": repetitions}),
                    observed: json!({
                        "completed": bundle.repeatability.completed,
                        "passed": bundle.repeatability.passed
                    }),
                    outcome: outcome(passed),
                    duration_ms: 0,
                    repetition: repetitions,
                });
        }
        if let Some(concurrency) = plan
            .selected_probe("todo_concurrent_create")
            .and_then(|check| check.concurrency)
        {
            let started = Instant::now();
            let (passed, observed) =
                run_concurrent_create(client, &self.contract, concurrency).await?;
            bundle
                .attempts
                .last_mut()
                .expect("runner always records an attempt")
                .probes
                .push(probe(
                    "todo_concurrent_create",
                    "bounded_concurrency",
                    json!({"concurrency": concurrency, "unique_ids": concurrency}),
                    observed,
                    outcome(passed),
                    started,
                    1,
                ));
        }

        let required = plan
            .compiled_checks
            .iter()
            .map(|check| check.probe_id.clone())
            .collect::<Vec<_>>();
        let covered = bundle
            .attempts
            .last()
            .into_iter()
            .flat_map(|attempt| &attempt.probes)
            .filter(|probe| probe.outcome != ProbeOutcome::NotEvaluated)
            .map(|probe| probe.id.clone())
            .collect::<BTreeSet<_>>();
        let omitted = required
            .iter()
            .filter(|probe| !covered.contains(*probe))
            .cloned()
            .collect::<Vec<_>>();
        bundle.coverage = ValidationCoverage {
            required,
            covered: covered.into_iter().collect(),
            complete: omitted.is_empty(),
            omitted,
        };
        if let Some(attempt) = bundle.attempts.last_mut() {
            attempt.verdict = if plan.compiled_checks.iter().all(|check| {
                attempt.probes.iter().any(|probe| {
                    probe.id == check.probe_id && probe.outcome == ProbeOutcome::Passed
                })
            }) {
                "passed".into()
            } else {
                "failed".into()
            };
        }
        Ok(bundle)
    }
}
