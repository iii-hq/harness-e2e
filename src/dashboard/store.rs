use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{JobStatus, RunMetadata};
use crate::report::E2eReport;

pub(super) struct StoredRun {
    pub(super) metadata: RunMetadata,
    pub(super) report: Option<E2eReport>,
    pub(super) live_progress: Option<super::live_progress::LiveProgress>,
    pub(super) live_progress_error: Option<String>,
}

#[cfg(test)]
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
    let report = read_report(run_dir)?;
    let metadata = match read_metadata(run_dir)? {
        Some(metadata) => metadata,
        None => {
            if let Some(report) = report.as_ref() {
                observed_metadata(run_dir, report)?
            } else {
                return Ok(None);
            }
        }
    };
    let (live_progress, live_progress_error) = if report.is_none() || metadata.status.active() {
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
        live_progress,
        live_progress_error,
    }))
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
        },
    })
}

#[cfg(test)]
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

    #[test]
    fn listing_rejects_unsupported_and_corrupt_results() {
        let root = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("old", br#"{"schema_version":2}"#.as_slice()),
            ("corrupt", b"not-json".as_slice()),
        ] {
            let directory = root.path().join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("results.json"), bytes).unwrap();
            assert!(read_stored_run(&directory).is_err());
        }
        assert!(load_runs(root.path()).unwrap().is_empty());
    }
}
