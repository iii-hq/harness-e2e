use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::workflow::WorkflowCheckpointV1;

/// Returns the most recently updated persisted checkpoint for read-only local tracking.
pub(super) fn latest_checkpoint(output_dir: &Path) -> Result<Option<WorkflowCheckpointV1>> {
    let root = output_dir.join("checkpoints");
    if !root.is_dir() {
        return Ok(None);
    }
    let mut latest: Option<WorkflowCheckpointV1> = None;
    for run in fs::read_dir(root)? {
        let run = run?.path();
        if !run.is_dir() {
            continue;
        }
        for attempt in fs::read_dir(run)? {
            let path = attempt?.path().join("workflow-checkpoint.json");
            if !path.is_file() {
                continue;
            }
            let checkpoint: WorkflowCheckpointV1 = serde_json::from_slice(&fs::read(&path)?)?;
            if latest
                .as_ref()
                .is_none_or(|observed| checkpoint.updated_at > observed.updated_at)
            {
                latest = Some(checkpoint);
            }
        }
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn latest_checkpoint_exposes_live_test_state() {
        let output = tempfile::tempdir().unwrap();
        let checkpoint = WorkflowCheckpointV1 {
            schema_version: 1,
            workflow_id: "security_review".into(),
            workflow_sha256: format!("sha256:{}", "a".repeat(64)),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            flow_snapshot: Value::Null,
            updated_at: "2026-08-17T12:00:00.000Z".into(),
            terminal_nodes: Vec::new(),
            active_nodes: vec!["scan_commit_a".into()],
            steps: Vec::new(),
        };
        crate::workflow::CheckpointStore::new(output.path(), "run", "attempt")
            .persist(&checkpoint)
            .unwrap();

        let observed = latest_checkpoint(output.path()).unwrap().unwrap();
        assert_eq!(observed.active_nodes, vec!["scan_commit_a"]);
        assert_eq!(observed.workflow_id, "security_review");
    }
}
