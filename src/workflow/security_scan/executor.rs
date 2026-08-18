use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum SecurityStepKind {
    ScanCommitA,
    SuggestCommitA,
    Reconciliation,
    ScheduledScanCommitB,
    ListRunHistory,
}

pub(super) struct SecurityExecutor {
    pub(super) context: Arc<E2eContext>,
    pub(super) kind: SecurityStepKind,
    pub(super) fixture: Arc<FixtureState>,
}

#[async_trait]
impl StepExecutor for SecurityExecutor {
    async fn preflight(&self, _context: &StepExecutorContext) -> Result<()> {
        for function in [
            REQUEST_FUNCTION,
            READ_FUNCTION,
            LIST_FUNCTION,
            RECONCILIATION_FUNCTION,
        ] {
            if !self.context.function_exists(function).await? {
                bail!("required security-scan function '{function}' is unavailable");
            }
        }
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        match self.kind {
            SecurityStepKind::ScanCommitA => self.scan_commit_a(&context).await,
            SecurityStepKind::SuggestCommitA => self.suggest_commit_a(&context).await,
            SecurityStepKind::Reconciliation => self.reconciliation(&context).await,
            SecurityStepKind::ScheduledScanCommitB => self.scheduled_scan_commit_b(&context).await,
            SecurityStepKind::ListRunHistory => self.list_run_history(&context).await,
        }
    }

    async fn evaluate(
        &self,
        context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        let mut output = execution.evaluation.clone();
        if output.hard_gates.is_empty() {
            output.hard_gates.push(WorkflowGateResult {
                id: format!("{}_completed", context.node.id),
                passed: true,
                reason: "The deterministic security-scan step completed.".into(),
                evidence_ids: Vec::new(),
            });
        }
        Ok(output)
    }
}
impl SecurityExecutor {
    async fn scan_commit_a(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let mut result = StepExecutorOutput::default();
        let preflight = self
            .preflight_fixture(&operation_context(
                context,
                json!({"repository": REPOSITORY, "scheduled_ref": SCHEDULED_REF}),
                BTreeMap::new(),
            ))
            .await?;
        let repository = operation_output_string(&preflight, "repository")?;
        let commit_a = operation_output_string(&preflight, "commit_a")?;
        append_operation(&mut result, preflight, "contracts");

        let request = self
            .request_scan(&operation_context(
                context,
                json!({"mode": "scan", "expect_deduplicated": null}),
                typed_inputs([
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                ]),
            ))
            .await?;
        let scan_run_id = operation_output_string(&request, "run_id")?;
        append_operation(&mut result, request, "request");

        let duplicate = self
            .request_scan(&operation_context(
                context,
                json!({"mode": "scan", "expect_deduplicated": true}),
                typed_inputs([
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                    ("original_run_id", text_value(scan_run_id.clone())),
                ]),
            ))
            .await?;
        append_operation(&mut result, duplicate, "duplicate_request");

        let waited = self
            .wait_run(&operation_context(
                context,
                json!({"expected_mode": "scan", "timeout_seconds": 360, "poll_interval_ms": 500}),
                typed_inputs([
                    ("run_id", text_value(scan_run_id.clone())),
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                ]),
            ))
            .await?;
        let report = operation_output_value(&waited, "report")?;
        let poll_metrics = waited.metrics.clone();
        append_operation(&mut result, waited, "run");

        let assessed = self
            .assess_report(&operation_context(
                context,
                json!({"mode": "scan", "seeded_paths": SEEDED_PATHS}),
                typed_inputs([("report", json_value(report.clone()))]),
            ))
            .await?;
        let findings_valid = operation_output_bool(&assessed, "findings_valid")?;
        let assessment = operation_output(&assessed, "assessment")?;
        let finding_count = report
            .get("findings")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let should_run_suggest = findings_valid && finding_count > 0;
        self.fixture.lock().suggest_expected = should_run_suggest;
        append_operation(&mut result, assessed, "report");

        let integrity = self
            .integrity(&operation_context(
                context,
                json!({"expected": "commit_a"}),
                BTreeMap::new(),
            ))
            .await?;
        append_operation(&mut result, integrity, "integrity");
        result.outputs = BTreeMap::from([
            ("repository".into(), text_value(repository)),
            ("commit_a".into(), text_value(commit_a)),
            ("scan_run_id".into(), text_value(scan_run_id)),
            ("report".into(), json_value(report)),
            ("should_run_suggest".into(), bool_value(should_run_suggest)),
            ("assessment".into(), assessment),
        ]);
        result.metrics = Some(json!({
            "request_count": 2,
            "finding_count": finding_count,
            "suggestion_branch_enabled": should_run_suggest,
            "poll": poll_metrics,
        }));
        Ok(result)
    }

    async fn suggest_commit_a(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let commit_a = input_string(context, "commit_a")?;
        let mut result = StepExecutorOutput::default();
        let request = self
            .request_scan(&operation_context(
                context,
                json!({"mode": "suggest", "expect_deduplicated": null}),
                typed_inputs([
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                ]),
            ))
            .await?;
        let run_id = operation_output_string(&request, "run_id")?;
        append_operation(&mut result, request, "request");
        let waited = self
            .wait_run(&operation_context(
                context,
                json!({"expected_mode": "suggest", "timeout_seconds": 360, "poll_interval_ms": 500}),
                typed_inputs([
                    ("run_id", text_value(run_id.clone())),
                    ("repository", text_value(repository)),
                    ("target_sha", text_value(commit_a)),
                ]),
            ))
            .await?;
        let report = operation_output_value(&waited, "report")?;
        let poll_metrics = waited.metrics.clone();
        append_operation(&mut result, waited, "run");
        let assessed = self
            .assess_report(&operation_context(
                context,
                json!({"mode": "suggest", "seeded_paths": SEEDED_PATHS}),
                typed_inputs([("report", json_value(report.clone()))]),
            ))
            .await?;
        let assessment = operation_output(&assessed, "assessment")?;
        append_operation(&mut result, assessed, "report");
        let integrity = self
            .integrity(&operation_context(
                context,
                json!({"expected": "commit_a"}),
                BTreeMap::new(),
            ))
            .await?;
        append_operation(&mut result, integrity, "integrity");
        result.outputs = BTreeMap::from([
            ("run_id".into(), text_value(run_id)),
            ("report".into(), json_value(report)),
            ("assessment".into(), assessment),
        ]);
        result.metrics = Some(json!({"request_count": 1, "poll": poll_metrics}));
        Ok(result)
    }

    async fn reconciliation(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let run_id = input_string(context, "scan_run_id")?;
        let mut result = StepExecutorOutput::default();
        let cached = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": false, "source": null, "severity": null, "limit": 100}),
                typed_inputs([("run_id", text_value(run_id.clone()))]),
            ))
            .await?;
        append_operation(&mut result, cached, "cached");
        let refreshed = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": true, "source": null, "severity": null, "limit": 100}),
                typed_inputs([("run_id", text_value(run_id.clone()))]),
            ))
            .await?;
        let snapshot = operation_output_value(&refreshed, "snapshot")?;
        append_operation(&mut result, refreshed, "refreshed");
        let reread = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": false, "source": null, "severity": null, "limit": 100}),
                typed_inputs([
                    ("run_id", text_value(run_id.clone())),
                    ("expected_snapshot", json_value(snapshot.clone())),
                ]),
            ))
            .await?;
        append_operation(&mut result, reread, "reread");
        let filtered = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": false, "source": "dependabot", "severity": "high", "limit": 1}),
                typed_inputs([("run_id", text_value(run_id))]),
            ))
            .await?;
        append_operation(&mut result, filtered, "filtered");
        result.outputs = BTreeMap::from([("snapshot".into(), json_value(snapshot))]);
        result.metrics = Some(json!({"reconciliation_operations": 4}));
        Ok(result)
    }

    async fn scheduled_scan_commit_b(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let mut result = StepExecutorOutput::default();
        let created = self
            .create_scheduled_commit(&operation_context(
                context,
                json!({"scheduled_ref": SCHEDULED_REF}),
                BTreeMap::new(),
            ))
            .await?;
        let commit_b = operation_output_string(&created, "commit_b")?;
        append_operation(&mut result, created, "scheduled_commit");
        let waited = self
            .wait_scheduled(&operation_context(
                context,
                json!({"timeout_seconds": 180, "poll_interval_ms": 500}),
                typed_inputs([
                    ("repository", text_value(repository)),
                    ("target_sha", text_value(commit_b.clone())),
                ]),
            ))
            .await?;
        let run_id = operation_output_string(&waited, "run_id")?;
        let report = operation_output_value(&waited, "report")?;
        let poll_metrics = waited.metrics.clone();
        append_operation(&mut result, waited, "scheduled_run");
        let assessed = self
            .assess_report(&operation_context(
                context,
                json!({"mode": "scan", "seeded_paths": SEEDED_PATHS}),
                typed_inputs([("report", json_value(report.clone()))]),
            ))
            .await?;
        let assessment = operation_output(&assessed, "assessment")?;
        append_operation(&mut result, assessed, "report");
        let integrity = self
            .integrity(&operation_context(
                context,
                json!({"expected": "commit_b"}),
                BTreeMap::new(),
            ))
            .await?;
        append_operation(&mut result, integrity, "integrity");
        result.outputs = BTreeMap::from([
            ("run_id".into(), text_value(run_id)),
            ("commit_b".into(), text_value(commit_b)),
            ("report".into(), json_value(report)),
            ("assessment".into(), assessment),
        ]);
        result.metrics = Some(json!({"cron_poll": poll_metrics}));
        Ok(result)
    }

    async fn list_run_history(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let suggest_expected = self.fixture.lock().suggest_expected;
        let expected_modes = if suggest_expected {
            json!(["scan", "suggest"])
        } else {
            json!(["scan"])
        };
        let mut output = self
            .list(&operation_context(
                context,
                json!({
                    "repository": REPOSITORY,
                    "status": "completed",
                    "limit": 100,
                    "expected_count": if suggest_expected { 3 } else { 2 },
                    "expected_modes": expected_modes,
                    "expect_suggest": suggest_expected,
                }),
                BTreeMap::new(),
            ))
            .await?;
        output.metrics = Some(json!({
            "listed_run_count": output.outputs.get("runs").and_then(|value| value.value.get("runs")).and_then(Value::as_array).map_or(0, Vec::len),
            "suggestion_expected": suggest_expected,
        }));
        Ok(output)
    }
}
