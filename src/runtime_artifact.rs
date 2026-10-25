//! Independent identity of the binaries exercised by a local suite.

use std::collections::BTreeMap;

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

/// The digest of every worker the two observations agree on: same process,
/// same start, same executable.
pub(crate) fn confirmed_digests(
    before: BTreeMap<String, Observation>,
    mut after: BTreeMap<String, Observation>,
) -> BTreeMap<String, String> {
    before
        .into_iter()
        .filter_map(|(worker, first)| {
            (after.remove(&worker).as_ref() == Some(&first)).then_some((worker, first.digest))
        })
        .collect()
}

/// The running Harness executable.
pub(crate) async fn observe(context: &E2eContext, engine_url: &str) -> Result<Observation> {
    observe_workers(context, engine_url)
        .await?
        .remove("harness")
        .context("Compose has no ready Harness process")
}

/// The running executable of every ready Compose container whose process
/// tree holds exactly one executable named after the container. Others
/// (scripts, runtimes, several or none) are left out, never guessed.
pub(crate) async fn observe_workers(
    context: &E2eContext,
    engine_url: &str,
) -> Result<BTreeMap<String, Observation>> {
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
    let ready: Vec<(String, u64)> = status["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|container| container["state"] == "ready")
        .filter_map(|container| {
            Some((
                container["container"].as_str()?.to_string(),
                container["pid"].as_u64()?,
            ))
        })
        .collect();
    tokio::task::spawn_blocking(move || {
        ready
            .into_iter()
            .filter_map(|(worker, pid)| {
                let observed = capture_compose(pid, &worker).ok()?;
                Some((worker, observed))
            })
            .collect()
    })
    .await
    .context("observe Compose executables")
}

fn named(executable: &std::path::Path, worker: &str) -> bool {
    executable
        .file_name()
        .is_some_and(|name| name == worker || *name == *format!("{worker} (deleted)"))
}

fn capture_compose(supervisor: u64, worker: &str) -> Result<Observation> {
    let started = start_ticks(supervisor)?;
    let mut pending = vec![(supervisor, None)];
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    while let Some((pid, expected_parent)) = pending.pop() {
        if !seen.insert(pid) {
            continue;
        }
        ensure!(
            seen.len() <= 256,
            "Compose process tree is too large to attest"
        );
        let (parent, discovered_start) = process_identity(pid)?;
        ensure!(
            expected_parent.is_none_or(|expected| parent == expected),
            "Compose child changed parent during capture"
        );
        let executable = std::fs::read_link(format!("/proc/{pid}/exe"))?;
        if named(&executable, worker) {
            let captured = capture(pid, worker)?;
            ensure!(
                captured.start_ticks == discovered_start,
                "{worker} PID was reused after discovery"
            );
            found.push(captured);
        }
        let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))?;
        for child in children.split_whitespace() {
            pending.push((child.parse()?, Some(pid)));
        }
    }
    ensure!(
        found.len() == 1,
        "Compose must own exactly one {worker} executable"
    );
    let observation = found.pop().context("the worker was not observed")?;
    let mut ancestor = observation.pid;
    for _ in 0..256 {
        if ancestor == supervisor || ancestor == 0 {
            break;
        }
        ancestor = process_identity(ancestor)?.0;
    }
    ensure!(
        ancestor == supervisor,
        "{worker} left the Compose process tree"
    );
    ensure!(
        start_ticks(observation.pid)? == observation.start_ticks,
        "{worker} PID was reused after capture"
    );
    ensure!(
        start_ticks(supervisor)? == started,
        "Compose supervisor PID was reused"
    );
    Ok(observation)
}

fn process_identity(pid: u64) -> Result<(u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // The command name may contain spaces and parentheses; fields 4 and 22.
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .context("invalid process stat")?
        .1
        .split_whitespace()
        .collect();
    Ok((
        fields.get(1).context("process parent is absent")?.parse()?,
        fields
            .get(19)
            .context("process start time is absent")?
            .parse()?,
    ))
}

fn start_ticks(pid: u64) -> Result<u64> {
    Ok(process_identity(pid)?.1)
}

fn capture(pid: u64, worker: &str) -> Result<Observation> {
    ensure!(pid > 0, "{worker} PID must be positive");
    let started = start_ticks(pid).with_context(|| format!("identify {worker} process"))?;
    let path = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
    let executable =
        std::fs::read_link(&path).with_context(|| format!("resolve {worker} executable"))?;
    ensure!(
        named(&executable, worker),
        "Compose PID is not a {worker} executable"
    );
    let bytes =
        std::fs::read(&path).with_context(|| format!("read running {worker} executable"))?;
    ensure!(
        start_ticks(pid)? == started,
        "{worker} PID was reused during capture"
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
        assert!(capture(0, "harness").is_err());
        assert!(capture(u64::MAX, "harness").is_err());
        assert!(capture(u64::from(std::process::id()), "harness").is_err());
    }

    #[test]
    fn any_worker_executable_named_after_its_container_is_attested() {
        let executable = std::env::current_exe().unwrap();
        let name = executable
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let own = capture(u64::from(std::process::id()), &name).unwrap();
        assert_eq!(
            own.digest,
            crate::artifact::sha256_bytes(&std::fs::read(&executable).unwrap())
        );
        assert!(capture(u64::from(std::process::id()), "context-manager").is_err());
    }

    #[test]
    fn only_workers_unchanged_across_the_suite_are_confirmed() {
        let at = |pid, digest: &str| Observation {
            pid,
            start_ticks: 1,
            digest: digest.into(),
        };
        let before = BTreeMap::from([
            ("harness".to_string(), at(1, "h")),
            ("context-manager".to_string(), at(2, "c")),
            ("llm-router".to_string(), at(3, "r")),
        ]);
        let after = BTreeMap::from([
            ("harness".to_string(), at(1, "h")),
            ("context-manager".to_string(), at(9, "c")),
        ]);
        assert_eq!(
            confirmed_digests(before, after),
            BTreeMap::from([("harness".to_string(), "h".to_string())])
        );
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
    fn a_compose_shell_wrapper_attests_the_harness_child_not_the_shell() {
        let directory = std::env::temp_dir().join(format!("e2e-wrapper-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("harness");
        std::fs::copy("/bin/bash", &executable).unwrap();
        let mut wrapper = std::process::Command::new("/bin/sh")
            .args(["-c", "\"$1\" -c 'sleep 30; :' & wait", "fixture"])
            .arg(&executable)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let observed = loop {
            match capture_compose(u64::from(wrapper.id()), "harness") {
                Ok(observed) => break observed,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("{error:#}"),
            }
        };
        assert_ne!(observed.pid, u64::from(wrapper.id()));
        // The same tree holds no context-manager: one worker's executable is
        // never attested as another's.
        assert!(capture_compose(u64::from(wrapper.id()), "context-manager").is_err());
        assert_eq!(
            observed.digest,
            crate::artifact::sha256_bytes(&std::fs::read(&executable).unwrap())
        );
        let children = std::fs::read_to_string(format!(
            "/proc/{}/task/{}/children",
            observed.pid, observed.pid
        ))
        .unwrap_or_default();
        for child in children.split_whitespace() {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", child])
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &observed.pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
        let _ = wrapper.kill();
        wrapper.wait().unwrap();
        std::fs::remove_file(executable).unwrap();
        std::fs::remove_dir(directory).unwrap();
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
        let captured = capture(u64::from(process.id()), "harness").unwrap();
        assert_eq!(captured.digest, expected);
        let _ = process.kill();
        process.wait().unwrap();
        std::fs::copy("/bin/bash", &executable).unwrap();
        let mut replacement = std::process::Command::new(&executable)
            .args(["-c", "read -t 30"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let after = capture(u64::from(replacement.id()), "harness").unwrap();
        let _ = replacement.kill();
        replacement.wait().unwrap();
        std::fs::remove_file(&executable).unwrap();
        std::fs::remove_dir(&directory).unwrap();
        assert_eq!(after.digest, expected);
        assert_eq!(confirmed_digest(Some(captured), Some(after)), None);
    }
}
