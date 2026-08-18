use super::evaluation::{candidate_gate_vector, decide, final_reconciliation_passes};
use super::helpers::*;
use super::schemas::*;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IncidentStepKind {
    Preflight,
    Baseline,
    Alert,
    Reproduce,
    ValidateTriage,
    ValidateDiagnosis,
    ValidateCandidate,
    Decide,
    Promote,
    Rollback,
    Reconcile,
    ValidateReport,
}

impl IncidentStepKind {
    fn functions(self) -> &'static [&'static str] {
        match self {
            Self::Preflight => &FIXTURE_FUNCTIONS,
            Self::Baseline => &[BASELINE_FUNCTION],
            Self::Alert => &[ALERT_FUNCTION],
            Self::Reproduce => &[REPRODUCE_FUNCTION, TELEMETRY_FUNCTION],
            Self::ValidateDiagnosis | Self::ValidateCandidate => &[VALIDATE_FUNCTION],
            Self::Promote | Self::Rollback => &[DEPLOY_FUNCTION],
            Self::Reconcile => &[RECONCILE_FUNCTION],
            Self::ValidateTriage | Self::Decide | Self::ValidateReport => &[],
        }
    }
}

pub(super) struct IncidentExecutor {
    pub(super) context: Arc<E2eContext>,
    pub(super) kind: IncidentStepKind,
    pub(super) fixture: Arc<IncidentFixtureState>,
}

#[async_trait]
impl StepExecutor for IncidentExecutor {
    async fn preflight(&self, _context: &StepExecutorContext) -> Result<()> {
        for function in self.kind.functions() {
            if !self.context.function_exists(function).await? {
                bail!("required incident fixture function '{function}' is unavailable");
            }
        }
        if self.kind == IncidentStepKind::Preflight {
            let path = fixture_path()?;
            validate_fixture_tree(&path)?;
        }
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        match self.kind {
            IncidentStepKind::Preflight => self.execute_preflight(&context).await,
            IncidentStepKind::Baseline => self.capture_baseline(&context).await,
            IncidentStepKind::Alert => self.deduplicate_alert(&context).await,
            IncidentStepKind::Reproduce => self.reproduce(&context).await,
            IncidentStepKind::ValidateTriage => self.validate_triage(&context).await,
            IncidentStepKind::ValidateDiagnosis => self.validate_diagnosis(&context).await,
            IncidentStepKind::ValidateCandidate => self.validate_candidate(&context).await,
            IncidentStepKind::Decide => self.decide_terminal(&context).await,
            IncidentStepKind::Promote => self.deploy(&context, "promote").await,
            IncidentStepKind::Rollback => self.deploy(&context, "rollback").await,
            IncidentStepKind::Reconcile => self.reconcile(&context).await,
            IncidentStepKind::ValidateReport => self.validate_report(&context).await,
        }
    }

    async fn evaluate(
        &self,
        context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        if !execution.evaluation.hard_gates.is_empty() {
            return Ok(execution.evaluation.clone());
        }
        Ok(StepEvaluation {
            hard_gates: vec![gate(
                &format!("{}_completed", context.node.id),
                true,
                "The deterministic incident-response step completed.",
                [],
            )],
            evaluations: execution.evaluation.evaluations.clone(),
        })
    }
}

impl IncidentExecutor {
    async fn execute_preflight(
        &self,
        _context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let path = fixture_path()?;
        ensure_clean(&path).await?;
        validate_fixture_tree(&path)?;
        let initial_head = git(&path, &["rev-parse", "HEAD"]).await?;
        let known_good_sha = git(
            &path,
            &[
                "rev-parse",
                crate::scenarios::incident_response::KNOWN_GOOD_REF,
            ],
        )
        .await?;
        let incident_sha = git(
            &path,
            &[
                "rev-parse",
                crate::scenarios::incident_response::INCIDENT_REF,
            ],
        )
        .await?;
        for sha in [&initial_head, &known_good_sha, &incident_sha] {
            validate_sha(sha)?;
        }
        let revisions_exact = initial_head == incident_sha && known_good_sha != incident_sha;

        let contract_path = path.join("fixture_contract.json");
        let contract: Value = serde_json::from_slice(
            &std::fs::read(&contract_path)
                .with_context(|| format!("read {}", contract_path.display()))?,
        )?;
        let expected_contract =
            crate::scenarios::incident_response::expected_fixture_contract_identity();
        let contract_exact = contract == expected_contract;
        let fixture_contract_sha256 = crate::artifact::sha256_value(&contract)?;

        let info = self
            .context
            .trigger_value(
                "engine::functions::info",
                json!({"function_ids": FIXTURE_FUNCTIONS}),
            )
            .await?;
        validate_contract_info(&info)?;
        let response: FixturePreflightResponse = self
            .context
            .trigger(
                PREFLIGHT_FUNCTION,
                FixturePreflightRequest {
                    workspace_root: path.to_string_lossy().into_owned(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        let worker_exact = response.repository == "iii-hq/incident-response-e2e-fixture"
            && response.workspace_root == path.to_string_lossy()
            && response.known_good_sha == known_good_sha
            && response.incident_sha == incident_sha
            && response.fixture_contract_sha256 == fixture_contract_sha256
            && response.clean
            && response.capability_version == "incident_fixture::v1";
        if !worker_exact {
            bail!("incident fixture worker returned contradictory preflight evidence");
        }
        self.fixture.lock().path = Some(path.clone());
        {
            let mut state = self.fixture.lock();
            state.initial_head = Some(initial_head.clone());
            state.known_good_sha = Some(known_good_sha.clone());
            state.incident_sha = Some(incident_sha.clone());
        }
        let evidence = json!({
            "repository": response.repository,
            "workspace_root": path,
            "initial_head": initial_head,
            "known_good_sha": known_good_sha,
            "incident_sha": incident_sha,
            "fixture_contract_sha256": fixture_contract_sha256,
            "hidden_probe_manifest_sha256": response.hidden_probe_manifest_sha256,
            "worker_contract_count": FIXTURE_FUNCTIONS.len(),
        });
        Ok(StepExecutorOutput {
            outputs: BTreeMap::from([
                ("workspace_root".into(), text_value(path.to_string_lossy())),
                ("preflight".into(), json_value(evidence)),
            ]),
            evaluation: StepEvaluation {
                hard_gates: vec![
                    gate("fixture_contract_exact", contract_exact, format!("fixture contract canonical hash={fixture_contract_sha256}"), []),
                    gate("fixture_revision_exact", revisions_exact, format!("HEAD={initial_head}, incident={incident_sha}, known_good={known_good_sha}"), []),
                    gate("fixture_clean_before_run", true, "Git status was clean before any workflow mutation.", []),
                    gate("worker_contracts_exact", true, "All nine fixture function schemas match incident_fixture::v1.", []),
                ],
                evaluations: Vec::new(),
            },
            ..StepExecutorOutput::default()
        })
    }

    async fn capture_baseline(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let incident_sha = state
            .incident_sha
            .context("preflight did not record incident SHA")?;
        let response: BaselineResponse = self
            .context
            .trigger(
                BASELINE_FUNCTION,
                BaselineRequest {
                    attempt_id: context.attempt_id.clone(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        let hashes_valid = [
            &response.data_sha256,
            &response.telemetry_sha256,
            &response.ledger_sha256,
            &response.audit_sha256,
        ]
        .iter()
        .all(|digest| digest.starts_with("sha256:") && digest.len() == 71);
        let complete = response.deployed_revision == incident_sha
            && response.active_operations == 0
            && hashes_valid;
        if !complete {
            bail!("fixture baseline is internally inconsistent");
        }
        let value = serde_json::to_value(&response)?;
        self.fixture.lock().baseline = Some(value.clone());
        let evidence = format!("{}.baseline_snapshot", context.node.id);
        Ok(output_with_asset(
            BTreeMap::from([("baseline".into(), json_value(value.clone()))]),
            asset(context.node.id.as_str(), "baseline_snapshot", "incident_baseline", value),
            vec![
                gate("baseline_captured_before_mutation", true, "Fixture reported zero active operations and the incident revision before alert submission.", [evidence.clone()]),
                gate("baseline_artifact_complete", true, "Deploy, data, telemetry, ledger, and audit identities are present.", [evidence]),
            ],
            Vec::new(),
        ))
    }

    async fn deduplicate_alert(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let key = format!("incident:{}:{}", context.run_id, context.attempt_id);
        let request = AlertRequest {
            event_id: crate::scenarios::incident_response::INCIDENT_EVENT_ID.into(),
            idempotency_key: key,
            _caller_worker_id: None,
        };
        let first: AlertResponse = self.context.trigger(ALERT_FUNCTION, &request).await?;
        let second: AlertResponse = self.context.trigger(ALERT_FUNCTION, &request).await?;
        let stable = !first.incident_id.is_empty()
            && first.incident_id == second.incident_id
            && first.alert_fingerprint == second.alert_fingerprint;
        let deduplicated = stable && second.deduplicated && second.request_count == 2;
        let value = json!({"first": first, "second": second});
        {
            let mut state = self.fixture.lock();
            state.incident_id = value
                .pointer("/second/incident_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            state.alert_fingerprint = value
                .pointer("/second/alert_fingerprint")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let evidence = format!("{}.incident_record", context.node.id);
        Ok(output_with_asset(
            BTreeMap::from([("incident".into(), json_value(value.clone()))]),
            asset(
                context.node.id.as_str(),
                "incident_record",
                "incident_record",
                value,
            ),
            vec![
                gate(
                    "alert_deduplicated",
                    deduplicated,
                    format!(
                        "second request deduplicated={}; request_count={}",
                        second.deduplicated, second.request_count
                    ),
                    [evidence.clone()],
                ),
                gate(
                    "incident_identity_stable",
                    stable,
                    format!("stable incident identity={stable}"),
                    [evidence],
                ),
            ],
            Vec::new(),
        ))
    }

    async fn reproduce(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let event_id = crate::scenarios::incident_response::INCIDENT_EVENT_ID.to_string();
        let first: ReproduceResponse = self
            .context
            .trigger(
                REPRODUCE_FUNCTION,
                ReproduceRequest {
                    event_id: event_id.clone(),
                    reproduction_key: format!("{}:{}:one", context.run_id, context.attempt_id),
                    _caller_worker_id: None,
                },
            )
            .await?;
        let second: ReproduceResponse = self
            .context
            .trigger(
                REPRODUCE_FUNCTION,
                ReproduceRequest {
                    event_id: event_id.clone(),
                    reproduction_key: format!("{}:{}:two", context.run_id, context.attempt_id),
                    _caller_worker_id: None,
                },
            )
            .await?;
        let repeated = first == second;
        let reproduced = first.event_id == event_id
            && first.attempts == 2
            && first.expected_settlement_count == 1
            && first.observed_settlement_count == 2
            && first.ledger_delta == 1;
        if !reproduced || !repeated {
            bail!("planned fixture reproduction was not injected consistently");
        }
        let mut telemetry = serde_json::Map::new();
        let mut evidence_ids = first.evidence_ids.iter().cloned().collect::<BTreeSet<_>>();
        for kind in ["logs", "metrics", "trace_change"] {
            let response: TelemetryResponse = self
                .context
                .trigger(
                    TELEMETRY_FUNCTION,
                    TelemetryRequest {
                        kind: kind.into(),
                        event_id: event_id.clone(),
                        _caller_worker_id: None,
                    },
                )
                .await?;
            if response.kind != kind || response.evidence_id.is_empty() {
                bail!("fixture returned contradictory {kind} telemetry");
            }
            if serde_json::to_vec(&response)?.len() > MAX_RESULT_BYTES as usize {
                bail!("fixture {kind} telemetry exceeds bounded result size");
            }
            evidence_ids.insert(response.evidence_id.clone());
            telemetry.insert(kind.into(), serde_json::to_value(response)?);
        }
        let workspace = self
            .fixture
            .snapshot()
            .path
            .context("preflight path missing")?;
        let results = result_root(&workspace, &context.run_id, &context.attempt_id);
        std::fs::create_dir_all(&results)?;
        let reproduction = json!({"first": first, "second": second});
        let bundle = json!({
            "result_root": results,
            "event_id": event_id,
            "reproduction": reproduction,
            "telemetry": telemetry,
            "allowed_evidence_ids": evidence_ids,
            "result_schema": result_contract_value::<AnalysisResult>()?,
        });
        {
            let mut state = self.fixture.lock();
            state.reproduction = Some(reproduction.clone());
            state.evidence_ids = evidence_ids;
        }
        let assessment = evaluation("incident_reproduction", true, "The seeded timeout/redelivery reproduced exactly two settlements where one was expected in two independent resets.", [format!("{}.reproduction_record", context.node.id)]);
        Ok(output_with_asset(
            BTreeMap::from([
                ("reproduction".into(), json_value(reproduction.clone())),
                ("analysis_bundle".into(), json_value(bundle)),
                ("assessment".into(), assessment_value(assessment.clone())?),
            ]),
            asset(context.node.id.as_str(), "reproduction_record", "incident_reproduction", reproduction),
            vec![
                gate("incident_reproduced", true, "Both seeded deliveries completed with the planned acknowledgement timeout.", [format!("{}.reproduction_record", context.node.id)]),
                gate("duplicate_effect_observed", true, "Observed settlement_count=2 for expected settlement_count=1.", [format!("{}.reproduction_record", context.node.id)]),
                gate("reproduction_repeatable", repeated, "Two fixture resets produced byte-equivalent reproduction records.", [format!("{}.reproduction_record", context.node.id)]),
                gate("reproduction_provenance_complete", !self.fixture.snapshot().evidence_ids.is_empty(), "Fixture evidence ids are present for logs, metrics, trace/change, and reproduction.", [format!("{}.reproduction_record", context.node.id)]),
            ],
            vec![assessment],
        ))
    }

    async fn validate_triage(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let workspace = state.path.context("preflight path missing")?;
        let expected = [
            ("analyze_logs", "logs"),
            ("analyze_metrics", "metrics"),
            ("analyze_trace_change", "trace_change"),
        ];
        let mut analyses = Vec::new();
        let mut failures = Vec::new();
        for (node, kind) in expected {
            match result_path(&workspace, &context.run_id, &context.attempt_id, node)
                .and_then(|path| load_result::<AnalysisResult>(&path))
            {
                Ok(result) => {
                    failures.extend(super::evaluation::validate_analysis(
                        &result,
                        kind,
                        &state.evidence_ids,
                    ));
                    analyses.push(result);
                }
                Err(error) => failures.push(format!("{node}: {error:#}")),
            }
        }
        let changes = production_changes(&workspace).await?;
        let read_only = changes.is_empty();
        if !read_only {
            failures.push(format!(
                "investigation changed repository paths: {}",
                changes.join(", ")
            ));
        }
        let valid = failures.is_empty() && analyses.len() == 3;
        let triage = json!({
            "result_root": result_root(&workspace, &context.run_id, &context.attempt_id),
            "analyses": analyses,
            "allowed_evidence_ids": state.evidence_ids,
            "reproduction": state.reproduction,
            "result_schema": result_contract_value::<DiagnosisResult>()?,
            "validation_failures": failures,
        });
        if valid {
            self.fixture.lock().triage = Some(triage.clone());
        }
        let evidence = format!("{}.triage_bundle", context.node.id);
        Ok(output_with_asset(
            BTreeMap::from([("triage".into(), json_value(triage.clone()))]),
            asset(context.node.id.as_str(), "triage_bundle", "incident_triage", triage),
            vec![
                gate("three_investigations_completed", analyses.len() == 3, format!("validated {} structured analysis files", analyses.len()), [evidence.clone()]),
                gate("investigations_parallel", true, "The validated code-owned graph makes all three nodes ready together under max_parallel=3.", [evidence.clone()]),
                gate("investigations_read_only", read_only, format!("non-evidence repository changes={}", changes.len()), [evidence.clone()]),
                gate("triage_results_schema_valid", valid, failures.join("; ").if_empty("all analysis files match their deny-unknown-fields contracts"), [evidence.clone()]),
                gate("triage_evidence_references_valid", valid, "Every cited evidence id belongs to the fixture-owned reproduction set.", [evidence.clone()]),
                gate("fan_in_after_all_analyses", analyses.len() == 3, "Scheduler dependencies require all three analysis nodes to succeed before this validator starts.", [evidence]),
            ],
            Vec::new(),
        ))
    }

    async fn validate_diagnosis(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let workspace = state.path.context("preflight path missing")?;
        let path = result_path(
            &workspace,
            &context.run_id,
            &context.attempt_id,
            "synthesize_diagnosis",
        )?;
        let parsed = load_result::<DiagnosisResult>(&path);
        let (diagnosis, failures) = match parsed {
            Ok(diagnosis) => {
                let analyses = state
                    .triage
                    .as_ref()
                    .and_then(|value| value.get("analyses"))
                    .cloned()
                    .and_then(|value| serde_json::from_value::<Vec<AnalysisResult>>(value).ok())
                    .unwrap_or_default();
                let failures = super::evaluation::validate_diagnosis(
                    &diagnosis,
                    &analyses,
                    &state.evidence_ids,
                );
                (Some(diagnosis), failures)
            }
            Err(error) => (None, vec![format!("{error:#}")]),
        };
        let mut probe_passed = false;
        if let Some(diagnosis) = &diagnosis {
            if failures.is_empty() {
                let response: ValidateResponse = self
                    .context
                    .trigger(
                        VALIDATE_FUNCTION,
                        ValidateRequest {
                            mode: "diagnosis".into(),
                            attempt_id: context.attempt_id.clone(),
                            workspace_root: workspace.to_string_lossy().into_owned(),
                            candidate_sha: None,
                            probe_ids: diagnosis.falsification_probe_ids.clone(),
                            _caller_worker_id: None,
                        },
                    )
                    .await?;
                probe_passed = diagnosis
                    .falsification_probe_ids
                    .iter()
                    .all(|id| response.probes.get(id).is_some_and(|probe| probe.passed));
            }
        }
        let changes = production_changes(&workspace).await?;
        let ready = failures.is_empty() && probe_passed && changes.is_empty();
        let record = json!({
            "diagnosis": diagnosis,
            "validation_failures": failures,
            "falsification_probe_passed": probe_passed,
            "diagnosis_precedes_mutation": changes.is_empty(),
            "result_root": result_root(&workspace, &context.run_id, &context.attempt_id),
            "allowed_path_patterns": crate::scenarios::incident_response::ALLOWED_PATH_PATTERNS,
            "protected_path_patterns": crate::scenarios::incident_response::PROTECTED_PATH_PATTERNS,
            "public_probe_ids": crate::scenarios::incident_response::PUBLIC_PROBE_IDS,
            "maximum_repair_rounds": crate::scenarios::incident_response::MAX_REPAIR_ROUNDS,
            "remediation_result_schema": result_contract_value::<RemediationResult>()?,
        });
        {
            let mut mutable = self.fixture.lock();
            mutable.diagnosis = diagnosis
                .as_ref()
                .and_then(|value| serde_json::to_value(value).ok());
            mutable.diagnosis_ready = ready;
        }
        let assessment = evaluation(
            "evidence_grounded_diagnosis",
            ready,
            if ready {
                "Diagnosis is grounded in validated triage and its fixture-owned falsification probes passed.".into()
            } else {
                format!(
                    "diagnosis invalid: {}; probe_passed={probe_passed}; changes={}",
                    failures.join("; "),
                    changes.len()
                )
            },
            [format!("{}.diagnosis_record", context.node.id)],
        );
        Ok(output_with_asset(
            BTreeMap::from([
                ("ready_for_remediation".into(), bool_value(ready)),
                ("diagnosis".into(), json_value(record.clone())),
                ("assessment".into(), assessment_value(assessment.clone())?),
            ]),
            asset(context.node.id.as_str(), "diagnosis_record", "incident_diagnosis", record),
            vec![
                gate("diagnosis_schema_valid", failures.is_empty(), failures.join("; ").if_empty("diagnosis matches the bounded structured contract"), [format!("{}.diagnosis_record", context.node.id)]),
                gate("diagnosis_grounded_in_reproduction", failures.is_empty(), "Selected hypothesis and evidence resolve to validated triage and reproduction evidence.", [format!("{}.diagnosis_record", context.node.id)]),
                gate("falsification_probe_executed", probe_passed, format!("fixture-owned falsification probes passed={probe_passed}"), [format!("{}.diagnosis_record", context.node.id)]),
                gate("diagnosis_precedes_mutation", changes.is_empty(), format!("production changes before remediation={}", changes.len()), [format!("{}.diagnosis_record", context.node.id)]),
            ],
            vec![assessment],
        ))
    }

    async fn validate_candidate(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let workspace = state.path.context("preflight path missing")?;
        let result = load_result::<RemediationResult>(&result_path(
            &workspace,
            &context.run_id,
            &context.attempt_id,
            "apply_remediation",
        )?);
        let summary = match result {
            Ok(value) if bounded_text(&value.summary, 4_000) && value.repair_rounds > 0 => value,
            Ok(_) => {
                return Ok(candidate_parse_failure(
                    "remediation result contains invalid bounded fields",
                ))
            }
            Err(error) => return Ok(candidate_parse_failure(&format!("{error:#}"))),
        };
        let validation: ValidateResponse = self
            .context
            .trigger(
                VALIDATE_FUNCTION,
                ValidateRequest {
                    mode: "candidate".into(),
                    attempt_id: context.attempt_id.clone(),
                    workspace_root: workspace.to_string_lossy().into_owned(),
                    candidate_sha: None,
                    probe_ids: vec![
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
                    .map(str::to_string)
                    .collect(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        if summary.repair_rounds != validation.repair_rounds {
            bail!("fixture validation repair round count contradicts remediation result");
        }
        if let Some(sha) = validation.candidate_sha.as_deref() {
            validate_sha(sha)?;
        }
        let vector = candidate_gate_vector(&validation);
        let valid = vector.iter().all(|(_, passed)| *passed) && validation.candidate_sha.is_some();
        self.fixture.lock().validation = Some(validation.clone());
        let mut hard_gates = vector
            .iter()
            .map(|(id, passed)| {
                gate(
                    id,
                    *passed,
                    format!("candidate validator observed {id}={passed}"),
                    [format!("{}.validation_matrix", context.node.id)],
                )
            })
            .collect::<Vec<_>>();
        hard_gates.push(gate(
            "candidate_sha_materialized",
            validation.candidate_sha.is_some(),
            format!("candidate_sha={:?}", validation.candidate_sha),
            [format!("{}.change_manifest", context.node.id)],
        ));
        let change_manifest = json!({
            "candidate_sha": validation.candidate_sha,
            "changed_paths": validation.changed_paths,
            "before_after_hashes": validation.before_after_hashes,
            "protected_paths_unchanged": validation.protected_paths_unchanged,
            "tests_unchanged": validation.tests_unchanged,
            "fixture_contract_unchanged": validation.fixture_contract_unchanged,
        });
        let validation_matrix =
            json!({"probes": validation.probes, "repair_rounds": validation.repair_rounds});
        Ok(StepExecutorOutput {
            outputs: BTreeMap::from([("candidate_valid".into(), bool_value(valid))]),
            captured_assets: vec![
                text_asset(
                    context.node.id.as_str(),
                    "remediation_patch",
                    "code_patch",
                    "text/x-diff; charset=utf-8",
                    validation.patch,
                ),
                asset(
                    context.node.id.as_str(),
                    "change_manifest",
                    "change_manifest",
                    change_manifest,
                ),
                asset(
                    context.node.id.as_str(),
                    "validation_matrix",
                    "validation_matrix",
                    validation_matrix,
                ),
            ],
            evaluation: StepEvaluation {
                hard_gates,
                evaluations: Vec::new(),
            },
            ..StepExecutorOutput::default()
        })
    }

    async fn decide_terminal(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let decision = decide(state.diagnosis_ready, state.validation.as_ref());
        if decision.should_promote == decision.should_rollback {
            bail!("terminal decision is not exclusive");
        }
        let (target, action) = if decision.should_promote {
            (
                state
                    .validation
                    .as_ref()
                    .and_then(|value| value.candidate_sha.clone())
                    .context("promotion has no candidate SHA")?,
                "promote",
            )
        } else {
            (
                state
                    .known_good_sha
                    .context("rollback has no known-good SHA")?,
                "rollback",
            )
        };
        {
            let mut mutable = self.fixture.lock();
            mutable.terminal_action = Some(action.into());
            mutable.terminal_revision = Some(target.clone());
        }
        let remediation_valid = state
            .validation
            .as_ref()
            .map(candidate_gate_vector)
            .is_some_and(|gates| gates.iter().all(|(_, passed)| *passed));
        let remediation = evaluation(
            "remediation_integrity",
            remediation_valid,
            format!("candidate deterministic gate vector passed={remediation_valid}"),
            [format!("{}.decision_record", context.node.id)],
        );
        let terminal = evaluation(
            "safe_terminal_action",
            true,
            format!("exclusive action={action}, target_revision={target}"),
            [format!("{}.decision_record", context.node.id)],
        );
        let record = json!({
            "diagnosis_ready": state.diagnosis_ready,
            "candidate_valid": remediation_valid,
            "should_promote": decision.should_promote,
            "should_rollback": decision.should_rollback,
            "action": action,
            "target_revision": target,
            "reason": decision.reason,
        });
        Ok(output_with_asset(
            BTreeMap::from([
                ("should_promote".into(), bool_value(decision.should_promote)),
                (
                    "should_rollback".into(),
                    bool_value(decision.should_rollback),
                ),
                (
                    "remediation_assessment".into(),
                    assessment_value(remediation.clone())?,
                ),
                (
                    "terminal_assessment".into(),
                    assessment_value(terminal.clone())?,
                ),
            ]),
            asset(
                context.node.id.as_str(),
                "decision_record",
                "incident_decision",
                record,
            ),
            vec![
                gate(
                    "terminal_decision_exclusive",
                    true,
                    format!(
                        "promote={}, rollback={}",
                        decision.should_promote, decision.should_rollback
                    ),
                    [format!("{}.decision_record", context.node.id)],
                ),
                gate(
                    "promotion_requires_all_gates",
                    !decision.should_promote || remediation_valid,
                    format!(
                        "promotion={}, candidate gates={remediation_valid}",
                        decision.should_promote
                    ),
                    [format!("{}.decision_record", context.node.id)],
                ),
            ],
            vec![remediation, terminal],
        ))
    }

    async fn deploy(
        &self,
        context: &StepExecutorContext,
        action: &str,
    ) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        if state.terminal_action.as_deref() != Some(action) {
            bail!("attempt-owned decision did not authorize {action}");
        }
        let revision = state
            .terminal_revision
            .context("terminal revision missing")?;
        let response: DeployResponse = self
            .context
            .trigger(
                DEPLOY_FUNCTION,
                DeployRequest {
                    action: action.into(),
                    revision: revision.clone(),
                    attempt_id: context.attempt_id.clone(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        if response.action != action
            || response.deployed_revision != revision
            || response.active_operations != 0
        {
            bail!("deploy simulator returned contradictory terminal action evidence");
        }
        Ok(StepExecutorOutput {
            evaluation: StepEvaluation {
                hard_gates: vec![
                    gate(
                        "one_terminal_operation",
                        true,
                        format!("executed exactly one {action} operation"),
                        [],
                    ),
                    gate(
                        if action == "promote" {
                            "promotion_uses_validated_sha"
                        } else {
                            "rollback_uses_known_good_sha"
                        },
                        true,
                        format!("deployed exact revision {revision}"),
                        [],
                    ),
                ],
                evaluations: Vec::new(),
            },
            ..StepExecutorOutput::default()
        })
    }

    async fn reconcile(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let expected = state
            .terminal_revision
            .context("terminal revision missing")?;
        let promoted = state.terminal_action.as_deref() == Some("promote");
        let response: ReconcileResponse = self
            .context
            .trigger(
                RECONCILE_FUNCTION,
                ReconcileRequest {
                    attempt_id: context.attempt_id.clone(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        let passed = final_reconciliation_passes(&response, &expected, promoted);
        let value = serde_json::to_value(&response)?;
        self.fixture.lock().final_state = Some(value.clone());
        let workspace = state.path.context("preflight path missing")?;
        let report_bundle = json!({
            "result_root": result_root(&workspace, &context.run_id, &context.attempt_id),
            "terminal_action": state.terminal_action,
            "target_revision": expected,
            "final_state": value,
            "evidence_ids": [
                "capture_baseline.baseline_snapshot",
                "deduplicate_alert.incident_record",
                "reproduce_incident.reproduction_record",
                "validate_triage.triage_bundle",
                "validate_diagnosis.diagnosis_record",
                "decide_terminal_action.decision_record",
                "reconcile_final_state.final_state"
            ],
            "result_schema": result_contract_value::<IncidentReportResult>()?,
        });
        let assessment = evaluation(
            "final_reconciliation",
            passed,
            format!("final fixture reconciliation passed={passed}"),
            [format!("{}.final_state", context.node.id)],
        );
        Ok(output_with_asset(
            BTreeMap::from([
                ("final_state".into(), json_value(value.clone())),
                ("report_bundle".into(), json_value(report_bundle)),
                ("assessment".into(), assessment_value(assessment.clone())?),
            ]),
            asset(
                context.node.id.as_str(),
                "final_state",
                "incident_final_state",
                value,
            ),
            vec![
                gate(
                    "deployed_revision_reconciled",
                    response.deployed_revision == expected,
                    format!(
                        "expected={expected}, observed={}",
                        response.deployed_revision
                    ),
                    [format!("{}.final_state", context.node.id)],
                ),
                gate(
                    "ledger_reconciled",
                    response.settlement_count <= 1 && response.distinct_events_preserved,
                    format!(
                        "settlements={}, distinct preserved={}",
                        response.settlement_count, response.distinct_events_preserved
                    ),
                    [format!("{}.final_state", context.node.id)],
                ),
                gate(
                    "incident_status_reconciled",
                    passed,
                    format!("incident_status={}", response.incident_status),
                    [format!("{}.final_state", context.node.id)],
                ),
                gate(
                    "no_active_fixture_operation",
                    response.active_operations == 0,
                    format!("active_operations={}", response.active_operations),
                    [format!("{}.final_state", context.node.id)],
                ),
                gate(
                    "evidence_captured_before_cleanup",
                    true,
                    "All domain assets were emitted by workflow steps before mandatory cleanup.",
                    [format!("{}.final_state", context.node.id)],
                ),
            ],
            vec![assessment],
        ))
    }

    async fn validate_report(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let state = self.fixture.snapshot();
        let workspace = state.path.context("preflight path missing")?;
        let parsed = load_result::<IncidentReportResult>(&result_path(
            &workspace,
            &context.run_id,
            &context.attempt_id,
            "write_incident_report",
        )?);
        let (markdown, error) = match parsed {
            Ok(value) if bounded_text(&value.markdown, 64 * 1024) => (Some(value.markdown), None),
            Ok(_) => (
                None,
                Some("incident report markdown is empty or oversized".to_string()),
            ),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        let expected_action = state.terminal_action.unwrap_or_default();
        let expected_revision = state.terminal_revision.unwrap_or_default();
        let required_evidence = [
            "reproduction_record",
            "diagnosis_record",
            "decision_record",
            "final_state",
        ];
        let references_valid = markdown.as_ref().is_some_and(|report| {
            report.contains(&expected_action)
                && report.contains(&expected_revision)
                && required_evidence.iter().all(|id| report.contains(id))
        });
        let mut assets = Vec::new();
        if let Some(markdown) = markdown {
            assets.push(text_asset(
                context.node.id.as_str(),
                "incident_report",
                "incident_report",
                "text/markdown; charset=utf-8",
                markdown,
            ));
        }
        Ok(StepExecutorOutput {
            outputs: BTreeMap::from([("validated".into(), bool_value(references_valid))]),
            captured_assets: assets,
            evaluation: StepEvaluation {
                hard_gates: vec![gate("incident_report_references_valid", references_valid, error.unwrap_or_else(|| format!("report references terminal action {expected_action}, revision {expected_revision}, and required evidence ids")), [format!("{}.incident_report", context.node.id)])],
                evaluations: vec![WorkflowEvaluationResult {
                    id: "incident_report_quality".into(),
                    outcome: WorkflowEvaluationOutcome::Advisory,
                    summary: "Report usefulness remains advisory; deterministic reference validation is authoritative.".into(),
                    score: None,
                    evidence_ids: vec![format!("{}.incident_report", context.node.id)],
                }],
            },
            ..StepExecutorOutput::default()
        })
    }
}

fn candidate_parse_failure(reason: &str) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs: BTreeMap::from([("candidate_valid".into(), bool_value(false))]),
        evaluation: StepEvaluation {
            hard_gates: vec![gate("candidate_result_schema_valid", false, reason, [])],
            evaluations: Vec::new(),
        },
        ..StepExecutorOutput::default()
    }
}

trait EmptyFallback {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyFallback for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.into()
        } else {
            self
        }
    }
}
