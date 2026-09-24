use std::fs;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

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
        .map(|scenario| scenario.aggregate.observed_runs)
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

/// The largest evidence file the Console reads in one call.
pub(super) const EVIDENCE_READ_LIMIT: u64 = 10 * 1024 * 1024;

/// One evidence file, or one screenshot inside a deliverable, as base64.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct EvidenceFile {
    pub media_type: String,
    pub base64: String,
}

/// A file a report declares for one of its runs, with the screenshots
/// embedded in it.
pub(super) struct DeclaredEvidence {
    pub path: String,
    pub media_type: String,
    pub screenshots: Vec<crate::screenshot::ScreenshotReference>,
}

/// Every evidence file and deliverable the report declares, retries included.
fn declared_evidence(report: &E2eReport) -> Vec<DeclaredEvidence> {
    let mut declared = Vec::new();
    let mut attempt = |evidence: &[crate::artifact::ArtifactReference],
                       deliverables: &[crate::report::DeliverableReport]| {
        for artifact in evidence {
            declared.push(DeclaredEvidence {
                path: artifact.path.clone(),
                media_type: artifact.media_type.clone(),
                screenshots: Vec::new(),
            });
        }
        for deliverable in deliverables {
            if let Some(artifact) = &deliverable.artifact {
                declared.push(DeclaredEvidence {
                    path: artifact.path.clone(),
                    media_type: artifact.media_type.clone(),
                    screenshots: deliverable.screenshots.clone(),
                });
            }
        }
    };
    for scenario in &report.scenarios {
        for run in &scenario.runs {
            attempt(&run.evidence, &run.deliverables);
            for retry in &run.retry_attempts {
                attempt(&retry.evidence, &retry.deliverables);
            }
        }
    }
    declared
}

/// One file the execution's report declares, read from the execution's own
/// directory; with a pointer, only that declared screenshot inside it.
pub(super) fn read_evidence(
    run_dir: &Path,
    path: &str,
    pointer: Option<&str>,
) -> Result<EvidenceFile> {
    let directory = report_directory(run_dir).context("This execution retained no report")?;
    let (report, results) = E2eReport::read_from(&directory)?;
    let root = results
        .parent()
        .context("results path has no parent directory")?;
    read_declared(root, &declared_evidence(&report), path, pointer)
}

/// The path must be one the report declares, relative, without `..`, and
/// still inside `root` once symlinks resolve; the file must fit the limit.
pub(super) fn read_declared(
    root: &Path,
    declared: &[DeclaredEvidence],
    path: &str,
    pointer: Option<&str>,
) -> Result<EvidenceFile> {
    crate::artifact::validate_relative_path(Path::new(path))?;
    let entry = declared
        .iter()
        .find(|entry| entry.path == path)
        .with_context(|| format!("The report does not declare evidence at '{path}'"))?;
    let base = fs::canonicalize(root)
        .with_context(|| format!("read execution directory {}", root.display()))?;
    let file = fs::canonicalize(root.join(path))
        .with_context(|| format!("Evidence '{path}' is not on disk"))?;
    ensure!(
        file.starts_with(&base) && file.is_file(),
        "Evidence '{path}' resolves outside its execution directory"
    );
    let size = fs::metadata(&file)?.len();
    ensure!(
        size <= EVIDENCE_READ_LIMIT,
        "Evidence '{path}' is {size} bytes; the Console reads at most {EVIDENCE_READ_LIMIT} bytes (10 MB)"
    );
    let bytes = fs::read(&file).with_context(|| format!("read evidence '{path}'"))?;
    let engine = base64::engine::general_purpose::STANDARD;
    let Some(pointer) = pointer else {
        return Ok(EvidenceFile {
            media_type: entry.media_type.clone(),
            base64: engine.encode(&bytes),
        });
    };
    let screenshot = entry
        .screenshots
        .iter()
        .find(|screenshot| screenshot.pointer == pointer)
        .with_context(|| format!("'{path}' declares no screenshot at '{pointer}'"))?;
    let content: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("decode deliverable '{path}'"))?;
    let encoded = content
        .pointer(pointer)
        .and_then(|file| file["content"].as_str())
        .with_context(|| format!("'{path}' holds no screenshot at '{pointer}'"))?;
    let image = engine
        .decode(encoded)
        .with_context(|| format!("decode screenshot '{pointer}'"))?;
    ensure!(
        crate::artifact::sha256_bytes(&image) == screenshot.sha256,
        "Screenshot '{pointer}' in '{path}' does not match its digest"
    );
    Ok(EvidenceFile {
        media_type: screenshot.media_type.clone(),
        base64: encoded.to_owned(),
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
    fn listing_rejects_unreadable_results() {
        let root = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (
                "partial",
                br#"{"result_contract_sha256":"sha256:foreign"}"#.as_slice(),
            ),
            ("corrupt", b"not-json".as_slice()),
        ] {
            let directory = root.path().join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("results.json"), bytes).unwrap();
            assert!(read_stored_run(&directory).is_err());
        }
        assert!(load_runs(root.path()).unwrap().is_empty());
    }

    fn screenshot(pointer: &str, image: &[u8]) -> crate::screenshot::ScreenshotReference {
        crate::screenshot::ScreenshotReference {
            pointer: pointer.into(),
            caption: "board".into(),
            media_type: "image/png".into(),
            sha256: crate::artifact::sha256_bytes(image),
            size_bytes: image.len() as u64,
        }
    }

    #[test]
    fn evidence_reads_only_declared_files_inside_the_execution() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let run = root.path().join("run");
        let png = b"\x89PNG\r\n\x1a\nimage";
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        fs::create_dir_all(run.join("deliverables")).unwrap();
        fs::write(run.join("evidence.json"), b"{}").unwrap();
        fs::write(
            run.join("deliverables/board.json"),
            serde_json::to_vec(&serde_json::json!({
                "attachments": {"board.png": {"encoding": "base64", "content": encoded}}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(outside.path().join("secret.json"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.json"), run.join("link.json"))
            .unwrap();
        let declared = |path: &str| DeclaredEvidence {
            path: path.into(),
            media_type: "application/json".into(),
            screenshots: vec![screenshot("/attachments/board.png", png)],
        };
        let declared = [
            declared("evidence.json"),
            declared("deliverables/board.json"),
            declared("link.json"),
            declared("../secret.json"),
            declared("/etc/passwd"),
        ];
        let read =
            |path: &str, pointer: Option<&str>| read_declared(&run, &declared, path, pointer);

        assert_eq!(read("evidence.json", None).unwrap().base64, "e30=");
        let image = read("deliverables/board.json", Some("/attachments/board.png")).unwrap();
        assert_eq!(
            (image.media_type.as_str(), image.base64.as_str()),
            ("image/png", encoded.as_str())
        );
        for (path, pointer, error) in [
            ("../secret.json", None, "parent"),
            ("/etc/passwd", None, "relative"),
            ("link.json", None, "outside its execution"),
            ("undeclared.json", None, "does not declare"),
            (
                "deliverables/board.json",
                Some("/attachments/other.png"),
                "declares no screenshot",
            ),
        ] {
            let message = format!("{:#}", read(path, pointer).unwrap_err());
            assert!(message.contains(error), "{path}: {message}");
        }
    }

    #[test]
    fn evidence_above_the_limit_is_refused_with_its_size() {
        let root = tempfile::tempdir().unwrap();
        let file = fs::File::create(root.path().join("large.json")).unwrap();
        file.set_len(EVIDENCE_READ_LIMIT + 1).unwrap();
        let declared = [DeclaredEvidence {
            path: "large.json".into(),
            media_type: "application/json".into(),
            screenshots: Vec::new(),
        }];
        let message = format!(
            "{:#}",
            read_declared(root.path(), &declared, "large.json", None).unwrap_err()
        );
        assert!(message.contains("10485761 bytes"), "{message}");
        assert!(message.contains("10 MB"), "{message}");
        file.set_len(EVIDENCE_READ_LIMIT).unwrap();
        assert!(read_declared(root.path(), &declared, "large.json", None).is_ok());
    }
}
