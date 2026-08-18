//! Held-out inputs and a way to run what the agent built.
//!
//! The trees these helpers write are created by the runner after the session
//! has ended, so a system that hard-codes answers for the sample it was shown
//! fails the moment it meets one of them.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::scenarios::deliverable::workspace;

pub(in crate::scenarios) struct Execution {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Execution {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(self.stdout.trim()).ok()
    }
}

/// A file the runner plants, plus the findings it should produce. Line numbers
/// are derived from the content, never counted by hand.
pub(in crate::scenarios) struct PlantedFile {
    pub path: &'static str,
    pub lines: &'static [&'static str],
    /// `(rule id, 1-based line)` for each issue this file carries.
    pub findings: &'static [(&'static str, usize)],
}

impl PlantedFile {
    fn contents(&self) -> String {
        let mut contents = self.lines.join("\n");
        contents.push('\n');
        contents
    }
}

pub(in crate::scenarios) fn plant(
    root: &Path,
    directory: &str,
    files: &[PlantedFile],
) -> anyhow::Result<()> {
    for file in files {
        workspace::write(
            root,
            &format!("{directory}/{}", file.path),
            &file.contents(),
        )?;
    }
    Ok(())
}

/// The findings a correct system must report for a planted tree, sorted so a
/// comparison never depends on the order they were emitted.
pub(in crate::scenarios) fn expected_findings(
    directory: &str,
    files: &[PlantedFile],
) -> Vec<(String, String, usize)> {
    let mut expected: Vec<(String, String, usize)> = files
        .iter()
        .flat_map(|file| {
            file.findings.iter().map(move |(rule, line)| {
                (
                    (*rule).to_string(),
                    format!("{directory}/{}", file.path),
                    *line,
                )
            })
        })
        .collect();
    expected.sort();
    expected
}

/// Findings as the built system reported them, normalised to the same shape:
/// paths relative to the workspace, line numbers as integers, sorted.
pub(in crate::scenarios) fn reported_findings(
    report: &serde_json::Value,
    directory: &str,
) -> Vec<(String, String, usize)> {
    let mut reported: Vec<(String, String, usize)> = report
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .map(|findings| {
            findings
                .iter()
                .filter_map(|finding| {
                    let rule = finding.get("rule")?.as_str()?.to_string();
                    let file = finding.get("file")?.as_str()?;
                    let line = usize::try_from(finding.get("line")?.as_u64()?).ok()?;
                    Some((rule, normalize(file, directory), line))
                })
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    reported.dedup();
    reported
}

/// Accept an absolute path, a workspace-relative path, or a path relative to
/// the scanned directory: they all name the same file.
fn normalize(file: &str, directory: &str) -> String {
    let file = file.replace('\\', "/");
    let marker = format!("{directory}/");
    match file.find(&marker) {
        Some(index) => file[index..].to_string(),
        None => format!("{directory}/{}", file.trim_start_matches("./")),
    }
}

pub(in crate::scenarios) async fn run(
    root: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<Execution> {
    let output = tokio::time::timeout(
        timeout,
        Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    Some(Execution {
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: &[PlantedFile] = &[PlantedFile {
        path: "app/config.py",
        lines: &["import os", "", "API_KEY = \"sk_live_0123456789\""],
        findings: &[("hardcoded_secret", 3)],
    }];

    #[test]
    fn expected_findings_carry_the_scanned_directory() {
        assert_eq!(
            expected_findings("holdout", FILES),
            vec![(
                "hardcoded_secret".to_string(),
                "holdout/app/config.py".to_string(),
                3
            )]
        );
    }

    #[test]
    fn a_report_is_accepted_however_it_spells_the_path() {
        for spelling in [
            "holdout/app/config.py",
            "./holdout/app/config.py",
            "/tmp/run/holdout/app/config.py",
        ] {
            let report = serde_json::json!({
                "findings": [{ "rule": "hardcoded_secret", "file": spelling, "line": 3 }]
            });
            assert_eq!(
                reported_findings(&report, "holdout"),
                expected_findings("holdout", FILES),
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_report_missing_fields_contributes_no_findings() {
        let report = serde_json::json!({ "findings": [{ "rule": "x", "file": "y" }] });
        assert!(reported_findings(&report, "holdout").is_empty());
    }
}
