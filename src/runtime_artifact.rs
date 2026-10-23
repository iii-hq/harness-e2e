//! Independent identity of the binary exercised by a local suite.

use anyhow::{ensure, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use serde_json::json;

use crate::context::E2eContext;

pub(crate) async fn observe(context: &E2eContext, engine_url: &str) -> Result<String> {
    ensure!(
        cfg!(target_os = "linux"),
        "executable capture requires Linux procfs"
    );
    let url = url::Url::parse(engine_url)?;
    ensure!(
        matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        ),
        "executable capture requires a local Engine"
    );
    let status = context
        .client()
        .trigger(TriggerRequest {
            function_id: "compose::status".into(),
            payload: json!({}),
            action: None,
            timeout_ms: Some(2_000),
        })
        .await
        .context("observe local Compose processes")?;
    let namespace = context
        .client()
        .namespace()
        .unwrap_or_else(|| "default".into());
    ensure!(
        status["namespace"] == namespace,
        "Compose namespace differs from the suite"
    );
    let pid = status["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|container| container["container"] == "harness" && container["state"] == "ready")
        .and_then(|container| container["pid"].as_u64())
        .context("Compose has no ready Harness process")?;
    tokio::task::spawn_blocking(move || capture(pid)).await?
}

fn capture(pid: u64) -> Result<String> {
    ensure!(pid > 0, "Harness PID must be positive");
    let path = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
    let executable = std::fs::read_link(&path).context("resolve Harness executable")?;
    ensure!(
        executable
            .file_name()
            .is_some_and(|name| name == "harness" || name == "harness (deleted)"),
        "Compose PID is not a Harness executable"
    );
    let bytes = std::fs::read(&path).context("read running Harness executable")?;
    Ok(crate::artifact::sha256_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_and_missing_processes_never_attest_harness() {
        assert!(capture(0).is_err());
        assert!(capture(u64::MAX).is_err());
        assert!(capture(u64::from(std::process::id())).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn captures_the_running_bytes_even_after_the_binary_is_replaced() {
        let directory = std::env::temp_dir().join(format!("e2e-binary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("harness");
        std::fs::copy("/bin/bash", &executable).unwrap();
        let expected = crate::artifact::sha256_bytes(&std::fs::read(&executable).unwrap());
        let mut process = std::process::Command::new(&executable)
            .args(["-c", "read -t 30"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::fs::remove_file(&executable).unwrap();
        let captured = capture(u64::from(process.id()));
        let _ = process.kill();
        process.wait().unwrap();
        std::fs::remove_dir(&directory).unwrap();
        assert_eq!(captured.unwrap(), expected);
    }
}
