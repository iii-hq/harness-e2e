use super::*;

pub(crate) fn evaluate_report(
    report: &Value,
    mode: &str,
    seeded_paths: &BTreeSet<&str>,
    fixture_path: Option<&Path>,
) -> Result<(Vec<WorkflowGateResult>, (usize, usize))> {
    let assessments = report.get("assessments").and_then(Value::as_object);
    let coverage_valid = assessments.is_some_and(|assessments| {
        ["vulnerabilities", "dependencies", "secrets", "supply_chain"]
            .iter()
            .all(|area| {
                assessments
                    .get(*area)
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
                    .is_some_and(|status| matches!(status, "assessed" | "not_assessed"))
            })
    });
    let findings = report
        .get("findings")
        .and_then(Value::as_array)
        .context("security report is missing findings[]")?;
    let mut paths_valid = true;
    let mut privacy_valid = true;
    let mut patch_policy_valid = true;
    let mut detected = BTreeSet::new();
    for finding in findings {
        if mode == "scan"
            && finding
                .get("suggested_patch")
                .is_some_and(|value| !value.is_null())
        {
            patch_policy_valid = false;
        }
        if contains_forbidden_key(finding) || contains_sensitive_string(finding, fixture_path) {
            privacy_valid = false;
        }
        if let Some(location) = finding.get("location").filter(|value| !value.is_null()) {
            let path = location
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if path.is_empty()
                || Path::new(path).is_absolute()
                || path.split('/').any(|part| part == "..")
            {
                paths_valid = false;
            }
            let start = location.get("line_start").and_then(Value::as_u64);
            let end = location.get("line_end").and_then(Value::as_u64);
            if start == Some(0)
                || end == Some(0)
                || start.zip(end).is_some_and(|(start, end)| end < start)
            {
                paths_valid = false;
            }
            if seeded_paths.contains(path) {
                detected.insert(path);
            }
        }
    }
    Ok((
        vec![
            gate(
                "security_area_coverage",
                coverage_valid,
                "All four security areas are explicitly assessed or not_assessed.",
            ),
            gate(
                "report_paths_and_lines",
                paths_valid,
                "Finding paths are relative and line ranges are valid.",
            ),
            gate(
                "public_report_privacy",
                privacy_valid,
                "Public output excludes internal roots, session ids and operation nonces.",
            ),
            gate(
                "mode_patch_policy",
                patch_policy_valid,
                "Scan mode does not contain suggested patches.",
            ),
        ],
        (detected.len(), seeded_paths.len()),
    ))
}

pub(crate) async fn evaluate_patch_applicability(
    report: &Value,
    fixture_path: &Path,
    step_id: &str,
) -> Result<WorkflowEvaluationResult> {
    let patches = report
        .get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|finding| finding.get("suggested_patch").and_then(Value::as_str))
        .filter(|patch| !patch.trim().is_empty())
        .collect::<Vec<_>>();
    if patches.is_empty() {
        return Ok(WorkflowEvaluationResult {
            id: "suggested_patch_applicability".into(),
            outcome: WorkflowEvaluationOutcome::NotEvaluated,
            summary: "Suggest mode produced no optional patch to check.".into(),
            score: None,
            evidence_ids: vec![format!("{step_id}.report")],
        });
    }

    let scratch =
        std::env::temp_dir().join(format!("harness-e2e-patch-check-{}", uuid::Uuid::new_v4()));
    let disposable = scratch.join("fixture");
    std::fs::create_dir(&scratch)
        .with_context(|| format!("create disposable patch root {}", scratch.display()))?;
    let disposable_text = disposable.to_string_lossy().into_owned();
    if let Err(error) = git(
        fixture_path,
        &["worktree", "add", "--detach", &disposable_text, "HEAD"],
    )
    .await
    {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(error.context("create disposable worktree for suggested patch checks"));
    }

    let mut applicable = 0_usize;
    let mut check_error = None;
    for (index, patch) in patches.iter().enumerate() {
        let patch_path = scratch.join(format!("candidate-{index}.patch"));
        if let Err(error) = std::fs::write(&patch_path, patch.as_bytes()) {
            check_error = Some(anyhow::Error::from(error).context(format!(
                "write disposable patch candidate {}",
                patch_path.display()
            )));
            break;
        }
        let patch_text = patch_path.to_string_lossy().into_owned();
        match git_success(&disposable, &["apply", "--check", &patch_text]).await {
            Ok(true) => applicable += 1,
            Ok(false) => {}
            Err(error) => {
                check_error = Some(error.context("run git apply --check"));
                break;
            }
        }
    }

    let remove_result = git(
        fixture_path,
        &["worktree", "remove", "--force", &disposable_text],
    )
    .await;
    let filesystem_cleanup = std::fs::remove_dir_all(&scratch);
    remove_result.context("remove disposable patch-check worktree")?;
    if let Err(error) = filesystem_cleanup {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error).context("remove disposable patch-check files");
        }
    }
    if let Some(error) = check_error {
        return Err(error);
    }

    Ok(WorkflowEvaluationResult {
        id: "suggested_patch_applicability".into(),
        outcome: if applicable == patches.len() {
            WorkflowEvaluationOutcome::Passed
        } else {
            WorkflowEvaluationOutcome::Advisory
        },
        summary: format!(
            "{applicable} of {} optional suggested patches passed git apply --check in a disposable worktree.",
            patches.len()
        ),
        score: Some(applicable as f64 / patches.len() as f64),
        evidence_ids: vec![format!("{step_id}.report")],
    })
}

pub(crate) fn evaluate_reconciliation(snapshot: &Value) -> Vec<WorkflowGateResult> {
    let sources = snapshot.get("sources").and_then(Value::as_array);
    let records = snapshot.get("records").and_then(Value::as_array);
    let scopes_valid = sources.is_some_and(|sources| {
        let scopes = sources
            .iter()
            .filter_map(|source| {
                Some((
                    source.get("source")?.as_str()?,
                    source.get("scope")?.as_str()?,
                ))
            })
            .collect::<BTreeMap<_, _>>();
        scopes.get("dependabot") == Some(&"repository_default_branch")
            && scopes.get("code_scanning") == Some(&"repository_snapshot")
            && snapshot
                .get("harness")
                .and_then(|value| value.get("scope"))
                .and_then(Value::as_str)
                == Some("exact_commit")
    });
    let records_valid = records.is_some_and(|records| {
        records.iter().all(|record| {
            record
                .get("public_url")
                .and_then(Value::as_str)
                .is_some_and(|url| url.starts_with("https://github.com/"))
                && !contains_forbidden_key(record)
        })
    });
    let counts_valid = sources.is_some_and(|sources| {
        sources.iter().all(|source| {
            let status = source
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let count = source.get("record_count");
            match status {
                "complete" | "partial" => count.is_some_and(Value::is_u64),
                "unavailable"
                | "authentication_required"
                | "permission_denied"
                | "disabled"
                | "not_configured"
                | "not_collected" => count.is_none_or(Value::is_null),
                _ => false,
            }
        })
    });
    vec![
        gate(
            "reconciliation_scopes",
            scopes_valid,
            "Harness and GitHub sources retain distinct scopes.",
        ),
        gate(
            "reconciliation_records",
            records_valid,
            "Reconciliation records use sanitized public GitHub URLs.",
        ),
        gate(
            "reconciliation_counts",
            counts_valid,
            "Source counts are nullable only when collection produced no usable data.",
        ),
    ]
}

pub(crate) fn evaluate_reconciliation_filters(
    snapshot: &Value,
    source: Option<&str>,
    severity: Option<&str>,
    limit: usize,
) -> WorkflowGateResult {
    let records = snapshot.get("records").and_then(Value::as_array);
    let valid = records.is_some_and(|records| {
        records.len() <= limit
            && records.iter().all(|record| {
                source.is_none_or(|expected| {
                    record.get("source").and_then(Value::as_str) == Some(expected)
                }) && severity.is_none_or(|expected| {
                    record.get("severity").and_then(Value::as_str) == Some(expected)
                })
            })
            && snapshot.get("next_cursor").is_none_or(|cursor| {
                cursor.is_null() || cursor.as_str().is_some_and(|value| !value.is_empty())
            })
    });
    gate(
        "reconciliation_filters_and_pagination",
        valid,
        "Filtered records respect source, severity, bounded page size and cursor shape.",
    )
}

pub(crate) fn reconciliation_infrastructure_failure(snapshot: &Value) -> Option<String> {
    let unavailable = snapshot
        .get("sources")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|source| {
            let status = source.get("status")?.as_str()?;
            (!matches!(status, "complete" | "partial")).then(|| {
                format!(
                    "{}={status}",
                    source
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )
            })
        })
        .collect::<Vec<_>>();
    (!unavailable.is_empty()).then(|| {
        format!(
            "GitHub reconciliation infrastructure did not collect every source: {}",
            unavailable.join(", ")
        )
    })
}

pub(crate) fn gate(id: &str, passed: bool, reason: &str) -> WorkflowGateResult {
    WorkflowGateResult {
        id: id.into(),
        passed,
        reason: reason.into(),
        evidence_ids: Vec::new(),
    }
}

pub(crate) fn contains_forbidden_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "operation_nonce" | "session_id" | "worktree_id"
            ) || contains_forbidden_key(value)
        }),
        Value::Array(values) => values.iter().any(contains_forbidden_key),
        _ => false,
    }
}

pub(crate) fn contains_sensitive_string(value: &Value, fixture_path: Option<&Path>) -> bool {
    match value {
        Value::String(text) => fixture_path
            .and_then(Path::to_str)
            .is_some_and(|root| !root.is_empty() && text.contains(root)),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_sensitive_string(value, fixture_path)),
        Value::Object(values) => values
            .values()
            .any(|value| contains_sensitive_string(value, fixture_path)),
        _ => false,
    }
}
