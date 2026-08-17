use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};

use super::{JobStatus, RunMetadata};
use crate::report::E2eReport;

pub(super) struct StoredRun {
    pub(super) metadata: RunMetadata,
    pub(super) report: Option<E2eReport>,
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
    let nested = run_dir.join("results");
    let path = if nested.join("results.json").is_file() {
        nested
    } else if run_dir.join("results.json").is_file() {
        run_dir.to_path_buf()
    } else {
        return Ok(None);
    };
    Ok(Some(E2eReport::read_from(&path)?.0))
}

pub(super) fn read_stored_run(run_dir: &Path) -> Result<Option<StoredRun>> {
    let report = read_report(run_dir)?;
    let metadata = match read_metadata(run_dir)? {
        Some(metadata) => metadata,
        None => {
            let Some(report) = report.as_ref() else {
                return Ok(None);
            };
            observed_metadata(run_dir, report)?
        }
    };
    Ok(Some(StoredRun { metadata, report }))
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
        let Some(run) = read_stored_run(&entry.path())? else {
            continue;
        };
        runs.push(run);
    }
    Ok(runs)
}
