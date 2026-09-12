use std::process::Stdio;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{io::AsyncWriteExt, process::Command};

use crate::persistence::Persistence;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct EvidenceRequest {
    pub execution_id: String,
    pub report_id: String,
    pub path: String,
}

impl Persistence {
    pub(crate) async fn open_history_evidence(&self, request: EvidenceRequest) -> Result<Value> {
        crate::artifact::validate_relative_path(std::path::Path::new(&request.path))?;
        let (_, execution) = self
            .imported_execution(&request.execution_id)
            .await?
            .context("Imported execution not found")?;
        let report = execution
            .reports
            .iter()
            .find(|r| r["id"] == request.report_id)
            .context("Report does not belong to the imported execution")?;
        if !report["payload"]["bundle"].is_null() {
            let bundle: super::GithubBundle =
                serde_json::from_value(report["payload"]["bundle"].clone())?;
            anyhow::ensure!(
                execution
                    .bundles
                    .iter()
                    .any(|retained| serde_json::to_value(retained).ok()
                        == serde_json::to_value(&bundle).ok()),
                "Report bundle is not part of the validated history references"
            );
        }
        let payload = serde_json::to_vec(
            &json!({"execution": execution.record, "report": report, "bundles": execution.bundles, "path": request.path}),
        )?;
        let child = Command::new("python3")
            .args(["-c", include_str!("../../scripts/open_history_evidence.py")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(_) => {
                return Ok(
                    json!({"availability": "access_unavailable", "reason": "Python 3 is required to read GitHub evidence locally."}),
                )
            }
        };
        child
            .stdin
            .take()
            .context("Evidence process has no input")?
            .write_all(&payload)
            .await?;
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Ok(
                json!({"availability": "access_unavailable", "reason": "The local evidence reader failed."}),
            );
        }
        serde_json::from_slice(&output.stdout).context("decode evidence availability")
    }
}
