use super::*;

impl SecurityExecutor {
    pub(super) async fn preflight_fixture(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let repository = config_string(context, "repository")?;
        let commit_b_ref = config_string(context, "commit_b_ref")?;
        let path = fixture_path()?;
        let head = git(&path, &["rev-parse", "HEAD"]).await?;
        validate_sha(&head)?;
        ensure_clean(&path).await?;
        if git_success(
            &path,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{commit_b_ref}"),
            ],
        )
        .await?
        {
            bail!("commit B ref '{commit_b_ref}' must not exist before the workflow");
        }
        let info = self
            .context
            .trigger_value(
                "engine::functions::info",
                json!({"function_ids": [REQUEST_FUNCTION, READ_FUNCTION, LIST_FUNCTION, RECONCILIATION_FUNCTION]}),
            )
            .await?;
        validate_contract_info(&info)?;
        {
            let mut fixture = self.fixture.lock();
            fixture.path = Some(path.clone());
            fixture.initial_head = Some(head.clone());
            fixture.commit_b_ref = Some(commit_b_ref);
        }
        let contracts = info.get("functions").cloned().unwrap_or(Value::Null);
        Ok(output_with_asset(
            BTreeMap::from([
                ("repository".into(), text_value(repository)),
                ("commit_a".into(), text_value(head)),
                ("contracts".into(), json_value(contracts.clone())),
            ]),
            "preflight",
            contracts,
            &context.node.id,
        ))
    }

    pub(super) async fn request_scan(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let target_sha = input_string(context, "target_sha")?;
        validate_sha(&target_sha)?;
        let mode = config_string(context, "mode")?;
        let expected_deduplicated = context
            .node
            .config
            .get("expect_deduplicated")
            .and_then(Value::as_bool);
        let response = self
            .context
            .trigger_value(
                REQUEST_FUNCTION,
                json!({"repository": repository, "target_sha": target_sha, "mode": mode}),
            )
            .await?;
        let run_id = required_string(&response, "run_id")?;
        let deduplicated = response
            .get("deduplicated")
            .and_then(Value::as_bool)
            .context("security-scan::request response is missing deduplicated")?;
        let original_run_id = context
            .inputs
            .get("original_run_id")
            .and_then(|value| value.value.as_str());
        let identity_matches = match (expected_deduplicated, original_run_id) {
            (Some(true), Some(original)) => original == run_id,
            (Some(true), None) => false,
            _ => true,
        };
        let deduplication_matches =
            expected_deduplicated.is_none_or(|expected| expected == deduplicated);
        let gate = WorkflowGateResult {
            id: "request_identity_and_deduplication".into(),
            passed: deduplication_matches && identity_matches,
            reason: format!(
                "Expected deduplicated={expected_deduplicated:?}; observed {deduplicated}; stable run identity={identity_matches}."
            ),
            evidence_ids: vec![format!("{}.request", context.node.id)],
        };
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([
                    ("run_id".into(), text_value(run_id)),
                    ("deduplicated".into(), bool_value(deduplicated)),
                    ("response".into(), json_value(response.clone())),
                ]),
                "request",
                response,
                &context.node.id,
            ),
            vec![gate],
            Vec::new(),
        ))
    }

    pub(super) async fn wait_run(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let run_id = input_string(context, "run_id")?;
        let repository = input_string(context, "repository")?;
        let target_sha = input_string(context, "target_sha")?;
        let expected_mode = config_string(context, "expected_mode")?;
        let timeout = Duration::from_secs(config_u64(context, "timeout_seconds")?);
        let interval = Duration::from_millis(config_u64(context, "poll_interval_ms")?);
        let started = Instant::now();
        let mut poll_count = 0_u64;
        loop {
            if *context.cancellation.borrow() {
                bail!("security-scan wait was cancelled");
            }
            let response = self
                .context
                .trigger_value(READ_FUNCTION, json!({"run_id": run_id}))
                .await?;
            poll_count += 1;
            let run = response
                .get("run")
                .filter(|value| !value.is_null())
                .cloned()
                .context("security-scan::read did not return the requested run")?;
            let status = required_string(&run, "status")?;
            if status == "completed" {
                let report = run
                    .get("report")
                    .cloned()
                    .context("completed security scan is missing report")?;
                let findings = report
                    .get("findings")
                    .and_then(Value::as_array)
                    .context("security report is missing findings[]")?;
                let has_findings = !findings.is_empty();
                let identity_valid = required_string(&run, "run_id")? == run_id
                    && required_string(&run, "repository")? == repository
                    && required_string(&run, "target_sha")? == target_sha
                    && required_string(&run, "mode")? == expected_mode;
                let mut output = output_with_internal_evaluation(
                    output_with_asset(
                        BTreeMap::from([
                            ("run".into(), json_value(run.clone())),
                            ("report".into(), json_value(report)),
                            ("has_findings".into(), bool_value(has_findings)),
                            ("run_id".into(), text_value(run_id)),
                        ]),
                        "run",
                        run,
                        &context.node.id,
                    ),
                    vec![gate(
                        "completed_run_identity",
                        identity_valid,
                        "Completed run retains the requested id, repository, full SHA and mode.",
                    )],
                    Vec::new(),
                );
                output.metrics = Some(json!({
                    "poll_count": poll_count,
                    "wait_duration_ms": started.elapsed().as_millis(),
                }));
                return Ok(output);
            }
            if matches!(status.as_str(), "failed" | "cancelled") {
                bail!(
                    "security scan run '{run_id}' ended as {status}: {}",
                    run.get("error").unwrap_or(&Value::Null)
                );
            }
            if started.elapsed() >= timeout {
                bail!(
                    "security scan run '{run_id}' did not complete within {}s",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(interval).await;
        }
    }

    pub(super) async fn assess_report(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let report = input_value(context, "report")?.clone();
        let mode = config_string(context, "mode")?;
        let seeded_paths = context
            .node
            .config
            .get("seeded_paths")
            .and_then(Value::as_array)
            .context("seeded_paths must be an array")?
            .iter()
            .map(|value| value.as_str().context("seeded path must be a string"))
            .collect::<Result<BTreeSet<_>>>()?;
        let (gates, capability) = evaluate_report(
            &report,
            &mode,
            &seeded_paths,
            fixture_path().ok().as_deref(),
        )?;
        let findings_valid = gates.iter().all(|gate| gate.passed);
        let mut evaluations = vec![WorkflowEvaluationResult {
            id: "seeded_vulnerability_detection".into(),
            outcome: if capability.0 == capability.1 {
                WorkflowEvaluationOutcome::Passed
            } else {
                WorkflowEvaluationOutcome::Advisory
            },
            summary: format!(
                "Detected {} of {} explicitly seeded vulnerable paths.",
                capability.0, capability.1
            ),
            score: (capability.1 > 0).then(|| capability.0 as f64 / capability.1 as f64),
            evidence_ids: vec![format!("{}.report", context.node.id)],
        }];
        if mode == "suggest" {
            evaluations.push(
                evaluate_patch_applicability(&report, &self.fixture.path()?, &context.node.id)
                    .await?,
            );
        }
        let assessment_value = serde_json::to_value(if mode == "suggest" {
            evaluations.last().expect("suggest evaluation is present")
        } else {
            evaluations
                .first()
                .expect("detection evaluation is present")
        })?;
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([
                    ("findings_valid".into(), bool_value(findings_valid)),
                    (
                        "assessment".into(),
                        TypedPortValue {
                            kind: PortValueKind::Assessment,
                            value: assessment_value,
                        },
                    ),
                    ("report".into(), json_value(report.clone())),
                ]),
                "report",
                report,
                &context.node.id,
            ),
            gates,
            evaluations,
        ))
    }

    pub(super) async fn reconciliation_operation(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let run_id = input_string(context, "run_id")?;
        let request = json!({
            "run_id": run_id,
            "refresh": config_bool(context, "refresh")?,
            "source": context.node.config.get("source").cloned().unwrap_or(Value::Null),
            "severity": context.node.config.get("severity").cloned().unwrap_or(Value::Null),
            "limit": config_u64(context, "limit")?,
        });
        let snapshot = self
            .context
            .trigger_value(RECONCILIATION_FUNCTION, request)
            .await?;
        let mut gates = evaluate_reconciliation(&snapshot);
        gates.push(evaluate_reconciliation_filters(
            &snapshot,
            context.node.config.get("source").and_then(Value::as_str),
            context.node.config.get("severity").and_then(Value::as_str),
            config_u64(context, "limit")? as usize,
        ));
        if let Some(expected) = context.inputs.get("expected_snapshot") {
            gates.push(gate(
                "reconciliation_cache_stable",
                expected.value == snapshot,
                "A non-refresh reread returns the exact durable sanitized snapshot.",
            ));
        }
        let technical_failure = config_bool(context, "refresh")?
            .then(|| reconciliation_infrastructure_failure(&snapshot))
            .flatten();
        let mut output = output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([("snapshot".into(), json_value(snapshot.clone()))]),
                "reconciliation",
                snapshot,
                &context.node.id,
            ),
            gates,
            Vec::new(),
        );
        output.technical_failure = technical_failure;
        Ok(output)
    }

    pub(super) async fn list(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = config_string(context, "repository")?;
        let status = context
            .node
            .config
            .get("status")
            .cloned()
            .unwrap_or(Value::Null);
        let response = self
            .context
            .trigger_value(
                LIST_FUNCTION,
                json!({
                    "repository": repository,
                    "status": status,
                    "limit": config_u64(context, "limit")?,
                }),
            )
            .await?;
        let runs = response
            .get("runs")
            .and_then(Value::as_array)
            .context("security-scan::list response is missing runs[]")?;
        let expected_count = config_u64(context, "expected_count")? as usize;
        let expected_modes = context.node.config["expected_modes"]
            .as_array()
            .context("expected_modes must be an array")?;
        let modes = runs
            .iter()
            .filter_map(|run| run.get("mode").and_then(Value::as_str))
            .collect::<BTreeSet<_>>();
        let expected_modes_present = expected_modes
            .iter()
            .filter_map(Value::as_str)
            .all(|mode| modes.contains(mode));
        let expected_status = status.as_str();
        let filters_valid = runs.len() <= config_u64(context, "limit")? as usize
            && runs.iter().all(|run| {
                run.get("repository").and_then(Value::as_str) == Some(repository.as_str())
                    && expected_status.is_none_or(|status| {
                        run.get("status").and_then(Value::as_str) == Some(status)
                    })
                    && run
                        .get("run_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
            });
        let (commit_a, commit_b) = {
            let fixture = self.fixture.lock();
            (fixture.initial_head.clone(), fixture.commit_b_sha.clone())
        };
        let expect_suggest = config_bool(context, "expect_suggest")?;
        let expected_lifecycle_present =
            commit_a.zip(commit_b).is_some_and(|(commit_a, commit_b)| {
                let mut expected = vec![(commit_a.as_str(), "scan"), (commit_b.as_str(), "scan")];
                if expect_suggest {
                    expected.push((commit_a.as_str(), "suggest"));
                }
                expected.into_iter().all(|(sha, mode)| {
                    runs.iter().any(|run| {
                        run.get("target_sha").and_then(Value::as_str) == Some(sha)
                            && run.get("mode").and_then(Value::as_str) == Some(mode)
                    })
                })
            });
        let gates = vec![WorkflowGateResult {
            id: "list_filters_and_integrity".into(),
            passed: runs.len() >= expected_count
                && expected_modes_present
                && filters_valid
                && expected_lifecycle_present,
            reason: format!("Observed {} runs and modes {:?}.", runs.len(), modes),
            evidence_ids: vec![format!("{}.list", context.node.id)],
        }];
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([("runs".into(), json_value(response.clone()))]),
                "list",
                response,
                &context.node.id,
            ),
            gates,
            Vec::new(),
        ))
    }

    pub(super) async fn integrity(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let path = self.fixture.path()?;
        let head = git(&path, &["rev-parse", "HEAD"]).await?;
        let status = git(
            &path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .await?;
        let expected = config_string(context, "expected")?;
        let expected_sha = {
            let fixture = self.fixture.lock();
            match expected.as_str() {
                "commit_a" => fixture.initial_head.clone(),
                "commit_b" => fixture.commit_b_sha.clone(),
                _ => None,
            }
        }
        .context("expected fixture commit has not been materialized")?;
        let snapshot = json!({"head": head, "status": status, "expected": expected_sha});
        let gates = vec![WorkflowGateResult {
            id: "fixture_immutable".into(),
            passed: head == expected_sha && status.is_empty(),
            reason: format!(
                "Fixture HEAD is {head}; worktree status is bounded and empty={}",
                status.is_empty()
            ),
            evidence_ids: vec![format!("{}.integrity", context.node.id)],
        }];
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([("snapshot".into(), json_value(snapshot.clone()))]),
                "integrity",
                snapshot,
                &context.node.id,
            ),
            gates,
            Vec::new(),
        ))
    }

    pub(super) async fn create_commit_b(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let path = self.fixture.path()?;
        ensure_clean(&path).await?;
        let commit_b_ref = config_string(context, "commit_b_ref")?;
        let marker = path.join("security-scan-e2e-commit-b.txt");
        std::fs::write(&marker, b"synthetic on-demand security scan fixture\n")
            .with_context(|| format!("write {}", marker.display()))?;
        git(&path, &["add", "--", "security-scan-e2e-commit-b.txt"]).await?;
        git(
            &path,
            &[
                "-c",
                "user.name=Harness E2E",
                "-c",
                "user.email=harness-e2e@example.invalid",
                "commit",
                "-m",
                "test: add on-demand security fixture",
            ],
        )
        .await?;
        let commit_b = git(&path, &["rev-parse", "HEAD"]).await?;
        validate_sha(&commit_b)?;
        git(
            &path,
            &[
                "update-ref",
                &format!("refs/heads/{commit_b_ref}"),
                &commit_b,
            ],
        )
        .await?;
        {
            let mut fixture = self.fixture.lock();
            fixture.commit_b_ref = Some(commit_b_ref);
            fixture.commit_b_sha = Some(commit_b.clone());
        }
        Ok(output_with_asset(
            BTreeMap::from([("commit_b".into(), text_value(commit_b.clone()))]),
            "commit-b",
            json!({"commit_b": commit_b}),
            &context.node.id,
        ))
    }
}
