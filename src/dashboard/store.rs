use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use super::{JobStatus, RunMetadata};
use crate::report::{E2eReport, RESULTS_SCHEMA_VERSION};

/// Historical report envelopes are discoverable, but never decoded as current
/// evidence or admitted to comparisons. The original files remain untouched.
#[derive(Debug, Serialize)]
pub(super) struct UnsupportedReport {
    pub(super) schema_version: u64,
    pub(super) expected_schema_version: u32,
    pub(super) result_contract_sha256: Option<String>,
}

pub(super) struct StoredRun {
    pub(super) metadata: RunMetadata,
    pub(super) report: Option<E2eReport>,
    pub(super) unsupported_report: Option<UnsupportedReport>,
    pub(super) live_progress: Option<super::live_progress::LiveProgress>,
    pub(super) live_progress_error: Option<String>,
}

pub(super) fn write_metadata(run_dir: &Path, metadata: &RunMetadata) -> Result<()> {
    fs::create_dir_all(run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let target = run_dir.join("metadata.json");
    let temporary = run_dir.join("metadata.json.tmp");
    let mut bytes = serde_json::to_vec_pretty(metadata)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, &target).with_context(|| format!("replace {}", target.display()))?;
    Ok(())
}

pub(super) fn read_metadata(run_dir: &Path) -> Result<Option<RunMetadata>> {
    let path = run_dir.join("metadata.json");
    if !path.is_file() {
        return Ok(None);
    }
    let value: RunMetadata = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("decode {}", path.display()))?;
    Ok(Some(value))
}

pub(super) fn read_report(run_dir: &Path) -> Result<Option<E2eReport>> {
    report_directory(run_dir)
        .map(|path| E2eReport::read_from(&path).map(|(report, _)| report))
        .transpose()
}

fn report_directory(run_dir: &Path) -> Option<std::path::PathBuf> {
    let nested = run_dir.join("results");
    if nested.join("results.json").is_file() {
        Some(nested)
    } else if run_dir.join("results.json").is_file() {
        Some(run_dir.to_path_buf())
    } else {
        None
    }
}

pub(super) fn read_stored_run(run_dir: &Path) -> Result<Option<StoredRun>> {
    let envelope = report_directory(run_dir)
        .map(|directory| -> Result<Value> {
            let path = directory.join("results.json");
            serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("decode result envelope {}", path.display()))
        })
        .transpose()?;
    let unsupported_report = envelope.as_ref().and_then(|value| {
        let version = value.get("schema_version")?.as_u64()?;
        (version > 0 && version != u64::from(RESULTS_SCHEMA_VERSION)).then(|| UnsupportedReport {
            schema_version: version,
            expected_schema_version: RESULTS_SCHEMA_VERSION,
            result_contract_sha256: value
                .get("result_contract_sha256")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    });
    let report = if unsupported_report.is_some() {
        None
    } else {
        // Current versions still require complete schema, manifest, and artifact
        // verification. Corruption must not be mistaken for incompatibility.
        read_report(run_dir)?
    };
    let metadata = match read_metadata(run_dir)? {
        Some(metadata) => metadata,
        None => {
            if let Some(report) = report.as_ref() {
                observed_metadata(run_dir, report)?
            } else if let Some(envelope) = envelope.as_ref() {
                unsupported_metadata(run_dir, envelope)?
            } else {
                return Ok(None);
            }
        }
    };
    let (live_progress, live_progress_error) = if unsupported_report.is_none()
        && (report.is_none() || metadata.status.active())
    {
        match super::live_progress::read(run_dir, &metadata.id) {
            Ok(progress) => (progress, None),
            Err(error) => {
                tracing::warn!(execution_id = %metadata.id, %error, "cannot verify live progress");
                (
                    None,
                    Some("Progress evidence could not be verified. Refresh to try again.".into()),
                )
            }
        }
    } else {
        (None, None)
    };
    Ok(Some(StoredRun {
        metadata,
        report,
        unsupported_report,
        live_progress,
        live_progress_error,
    }))
}

fn unsupported_metadata(run_dir: &Path, envelope: &Value) -> Result<RunMetadata> {
    let field = |pointer: &str| -> Result<String> {
        envelope
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .with_context(|| format!("historical result has no valid {pointer}"))
    };
    let id = field("/execution/execution_id")?;
    if run_dir.file_name().and_then(|value| value.to_str()) != Some(id.as_str()) {
        bail!("historical result execution id does not match its directory");
    }
    let started_at = field("/execution/started_at")?;
    let completed_at = field("/execution/completed_at")?;
    for timestamp in [&started_at, &completed_at] {
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .context("historical result has an invalid execution timestamp")?;
    }
    // This is only a discovery envelope. Do not reinterpret historical scores,
    // run counts, outcomes, artifact references, or comparison identities.
    Ok(RunMetadata {
        id,
        label: "e2e::* historical run".into(),
        status: JobStatus::Completed,
        started_at,
        completed_at,
        returncode: None,
        error: String::new(),
        request: super::RunRequest {
            _caller_worker_id: None,
            label: "e2e::* historical run".into(),
            url: String::new(),
            model: field("/subject/model")?,
            provider: field("/subject/provider")?,
            judge_model: String::new(),
            judge_provider: String::new(),
            scenarios: Vec::new(),
            runs: 0,
            technical_retries: 0,
            seed: None,
            plan_context: None,
        },
        plan_context: None,
    })
}

fn observed_metadata(run_dir: &Path, report: &E2eReport) -> Result<RunMetadata> {
    let execution = &report.execution;
    let directory_id = run_dir
        .file_name()
        .and_then(|value| value.to_str())
        .context("control-plane run directory has no UTF-8 name")?;
    if directory_id != execution.execution_id {
        bail!(
            "control-plane run directory {} does not match execution id {}",
            directory_id,
            execution.execution_id
        );
    }
    let requested_runs = report
        .scenarios
        .iter()
        .map(|scenario| scenario.aggregate.runs)
        .max()
        .unwrap_or(1);
    let seed = report
        .scenarios
        .iter()
        .filter_map(|scenario| scenario.case.as_ref().map(|case| case.seed))
        .next();
    Ok(RunMetadata {
        id: execution.execution_id.clone(),
        label: "e2e::* control-plane run".into(),
        status: JobStatus::Completed,
        started_at: execution.started_at.clone(),
        completed_at: execution.completed_at.clone(),
        returncode: Some(0),
        error: String::new(),
        request: super::RunRequest {
            _caller_worker_id: None,
            label: "e2e::* control-plane run".into(),
            url: String::new(),
            model: report.subject.model.clone(),
            provider: report.subject.provider.clone(),
            judge_model: report
                .judge
                .as_ref()
                .map(|judge| judge.model.clone())
                .unwrap_or_default(),
            judge_provider: report
                .judge
                .as_ref()
                .map(|judge| judge.provider.clone())
                .unwrap_or_default(),
            scenarios: report
                .scenarios
                .iter()
                .map(|scenario| scenario.scenario_id.as_str().to_string())
                .collect(),
            runs: requested_runs,
            technical_retries: 0,
            seed,
            plan_context: None,
        },
        plan_context: None,
    })
}

pub(super) fn recover_interrupted_runs(runs_dir: &Path) -> Result<Vec<RunMetadata>> {
    let mut recovered = Vec::new();
    for entry in fs::read_dir(runs_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(mut metadata) = read_metadata(&entry.path())? else {
            continue;
        };
        if metadata.status.active() {
            metadata.status = JobStatus::Failed;
            metadata.completed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            metadata.error = "dashboard stopped before the runner completed".into();
            write_metadata(&entry.path(), &metadata)?;
            recovered.push(metadata);
        }
    }
    Ok(recovered)
}

pub(super) fn load_runs(runs_dir: &Path) -> Result<Vec<StoredRun>> {
    let mut runs = Vec::new();
    for entry in fs::read_dir(runs_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        match read_stored_run(&entry.path()) {
            Ok(Some(run)) => runs.push(run),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                path = %entry.path().display(),
                error = %format!("{error:#}"),
                "ignoring a corrupt or unreadable E2E execution"
            ),
        }
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn historical_envelope(id: &str, version: u32) -> Value {
        json!({
            "schema_version": version,
            "execution": {
                "execution_id": id,
                "started_at": "2026-09-01T12:00:00Z",
                "completed_at": "2026-09-01T13:00:00Z",
            },
            "subject": {"model": "historical-model", "provider": "provider"},
            // These historical fields must never become current metrics.
            "passed": true,
            "scenarios": [{"aggregate": {"runs": 100, "score": 100}}],
        })
    }

    fn write_envelope(run_dir: &Path, value: &Value) {
        fs::create_dir_all(run_dir).unwrap();
        fs::write(
            run_dir.join("results.json"),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn v2_and_v3_history_stays_visible_without_reinterpreting_its_metrics() {
        let root = tempfile::tempdir().unwrap();
        for version in [2, 3] {
            assert_ne!(version, RESULTS_SCHEMA_VERSION);
            let id = format!("legacy-v{version}");
            let run_dir = root.path().join(&id);
            let envelope = historical_envelope(&id, version);
            write_envelope(&run_dir, &envelope);
            let original = fs::read(run_dir.join("results.json")).unwrap();
            let stored = read_stored_run(&run_dir).unwrap().unwrap();
            assert!(stored.report.is_none());
            assert!(stored.live_progress.is_none());
            assert!(stored.live_progress_error.is_none());
            assert_eq!(
                stored.unsupported_report.as_ref().unwrap().schema_version,
                u64::from(version),
            );
            for projection in [
                super::super::presenter::stored_execution_summary(&stored).unwrap(),
                super::super::presenter::stored_execution_detail(&stored).unwrap(),
            ] {
                assert_eq!(projection["id"], id);
                assert_eq!(projection["status"], "unsupported");
                assert_eq!(projection["availability"], "unsupported");
                assert_eq!(projection["baseline_comparable"], false);
                assert_eq!(projection["conclusion"], "");
                assert_eq!(projection["execution"]["conclusion"], "");
                assert_eq!(projection["subjects"][0]["model"], "historical-model");
                assert_eq!(projection["subjects"][0]["scenarios"], json!([]));
                assert_eq!(projection["totals"], json!({}));
                assert_eq!(projection["scenario_metrics"], json!([]));
                assert!(projection["requested_runs"].is_null());
                assert_eq!(
                    projection["result_compatibility"]["schema_version"],
                    version,
                );
                assert_eq!(
                    projection["first_failure"]["kind"],
                    "unsupported_results_schema",
                );
            }
            let detail = super::super::presenter::stored_execution_detail(&stored).unwrap();
            assert_eq!(detail["reports"], json!([]));
            assert_eq!(fs::read(run_dir.join("results.json")).unwrap(), original);
            assert!(!run_dir.join("metadata.json").exists());
        }
        assert_eq!(load_runs(root.path()).unwrap().len(), 2);
        let model = super::super::read_model::DashboardReadModel::load(root.path()).unwrap();
        assert_eq!(model.summaries.len(), 2);
        let versions = model.evaluated_versions(Default::default());
        assert!(versions.cohorts.is_empty());
        assert!(versions.versions.is_empty());
    }

    #[test]
    fn unsupported_nested_result_uses_existing_metadata_without_assuming_an_envelope() {
        let root = tempfile::tempdir().unwrap();
        let run_dir = root.path().join("legacy");
        let metadata = unsupported_metadata(&run_dir, &historical_envelope("legacy", 2)).unwrap();
        write_metadata(&run_dir, &metadata).unwrap();
        write_envelope(&run_dir.join("results"), &json!({"schema_version": 2}));
        let stored = read_stored_run(&run_dir).unwrap().unwrap();
        assert_eq!(stored.metadata.id, "legacy");
        assert_eq!(stored.metadata.request.model, "historical-model");
        assert!(stored.unsupported_report.is_some());
        assert!(stored.report.is_none());
    }

    #[test]
    fn unsupported_schema_does_not_make_a_missing_or_mismatched_identity_discoverable() {
        let root = tempfile::tempdir().unwrap();
        let run_dir = root.path().join("legacy");
        for envelope in [
            json!({"schema_version": 2}),
            historical_envelope("different-execution", 2),
        ] {
            write_envelope(&run_dir, &envelope);
            assert!(read_stored_run(&run_dir).is_err());
            assert!(load_runs(root.path()).unwrap().is_empty());
        }
    }

    #[test]
    fn current_but_invalid_results_are_corrupt_not_unsupported_history() {
        let root = tempfile::tempdir().unwrap();
        let run_dir = root.path().join("current");
        write_envelope(
            &run_dir,
            &historical_envelope("current", RESULTS_SCHEMA_VERSION),
        );
        assert!(read_stored_run(&run_dir).is_err());
        assert!(load_runs(root.path()).unwrap().is_empty());
    }

    #[test]
    fn malformed_results_remain_corrupt_even_with_valid_metadata() {
        let root = tempfile::tempdir().unwrap();
        let run_dir = root.path().join("legacy");
        let metadata = unsupported_metadata(&run_dir, &historical_envelope("legacy", 2)).unwrap();
        write_metadata(&run_dir, &metadata).unwrap();
        fs::write(run_dir.join("results.json"), b"not-json\n").unwrap();
        assert!(read_stored_run(&run_dir).is_err());
        assert!(load_runs(root.path()).unwrap().is_empty());
    }

    #[test]
    fn listing_isolates_a_corrupt_execution_directory() {
        let root = tempfile::tempdir().expect("temporary directory");
        let corrupt = root.path().join("corrupt-execution");
        fs::create_dir_all(&corrupt).expect("execution directory");
        fs::write(corrupt.join("results.json"), b"not-json\n").expect("corrupt result fixture");

        assert!(load_runs(root.path()).unwrap().is_empty());
    }
}
