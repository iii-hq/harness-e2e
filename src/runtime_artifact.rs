//! Independent identity of the binary exercised by a local suite.

use anyhow::{ensure, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use serde_json::json;

use crate::context::E2eContext;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pid: u64,
    start_ticks: u64,
    pub(crate) digest: String,
}

pub(crate) fn confirmed_digest(
    before: Option<Observation>,
    after: Option<Observation>,
) -> Option<String> {
    before
        .filter(|first| Some(first) == after.as_ref())
        .map(|same| same.digest)
}

pub(crate) async fn observe(context: &E2eContext, engine_url: &str) -> Result<Observation> {
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

fn start_ticks(pid: u64) -> Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // The command name may contain spaces and parentheses. Field 22 is starttime.
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .context("process start time is absent")?
        .parse()
        .context("invalid process start time")
}

fn capture(pid: u64) -> Result<Observation> {
    ensure!(pid > 0, "Harness PID must be positive");
    let started = start_ticks(pid).context("identify Harness process")?;
    let path = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
    let executable = std::fs::read_link(&path).context("resolve Harness executable")?;
    ensure!(
        executable
            .file_name()
            .is_some_and(|name| name == "harness" || name == "harness (deleted)"),
        "Compose PID is not a Harness executable"
    );
    let bytes = std::fs::read(&path).context("read running Harness executable")?;
    ensure!(
        start_ticks(pid)? == started,
        "Harness PID was reused during capture"
    );
    Ok(Observation {
        pid,
        start_ticks: started,
        digest: crate::artifact::sha256_bytes(&bytes),
    })
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
    fn only_an_unchanged_process_can_attest_a_suite() {
        let sample = || Observation {
            pid: 10,
            start_ticks: 20,
            digest: "abc".into(),
        };
        assert_eq!(
            confirmed_digest(Some(sample()), Some(sample())),
            Some("abc".into())
        );
        assert_eq!(confirmed_digest(None, Some(sample())), None);
        assert_eq!(confirmed_digest(Some(sample()), None), None);
        for changed in [
            Observation {
                pid: 11,
                ..sample()
            },
            Observation {
                start_ticks: 21,
                ..sample()
            },
            Observation {
                digest: "def".into(),
                ..sample()
            },
        ] {
            assert_eq!(confirmed_digest(Some(sample()), Some(changed)), None);
        }
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
        let captured = capture(u64::from(process.id())).unwrap();
        assert_eq!(captured.digest, expected);
        let _ = process.kill();
        process.wait().unwrap();
        std::fs::copy("/bin/bash", &executable).unwrap();
        let mut replacement = std::process::Command::new(&executable)
            .args(["-c", "read -t 30"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let after = capture(u64::from(replacement.id())).unwrap();
        let _ = replacement.kill();
        replacement.wait().unwrap();
        std::fs::remove_file(&executable).unwrap();
        std::fs::remove_dir(&directory).unwrap();
        assert_eq!(after.digest, expected);
        assert_eq!(confirmed_digest(Some(captured), Some(after)), None);
    }
}
