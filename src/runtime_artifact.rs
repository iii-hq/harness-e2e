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
    tokio::task::spawn_blocking(move || capture_compose(pid)).await?
}

fn capture_compose(supervisor: u64) -> Result<Observation> {
    let started = start_ticks(supervisor)?;
    let mut pending = vec![(supervisor, None)];
    let mut seen = std::collections::HashSet::new();
    let mut harnesses = Vec::new();
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
        if executable
            .file_name()
            .is_some_and(|name| name == "harness" || name == "harness (deleted)")
        {
            let captured = capture(pid)?;
            ensure!(
                captured.start_ticks == discovered_start,
                "Harness PID was reused after discovery"
            );
            harnesses.push(captured);
        }
        // Children of every thread, not only the main one: a second Harness
        // started from another thread must count too.
        for task in std::fs::read_dir(format!("/proc/{pid}/task"))? {
            let children = match std::fs::read_to_string(task?.path().join("children")) {
                Ok(children) => children,
                // The thread ended between the listing and the read.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for child in children.split_whitespace() {
                pending.push((child.parse()?, Some(pid)));
            }
        }
    }
    ensure!(
        harnesses.len() == 1,
        "Compose must own exactly one Harness executable"
    );
    let observation = harnesses.pop().context("Harness was not observed")?;
    let mut ancestor = observation.pid;
    for _ in 0..256 {
        if ancestor == supervisor || ancestor == 0 {
            break;
        }
        ancestor = process_identity(ancestor)?.0;
    }
    ensure!(
        ancestor == supervisor,
        "Harness left the Compose process tree"
    );
    ensure!(
        start_ticks(observation.pid)? == observation.start_ticks,
        "Harness PID was reused after capture"
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

    /// A Harness started from a thread other than the supervisor's main one
    /// is found, and one hidden beside the main thread's Harness makes it
    /// unattested.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_harness_started_from_any_thread_counts_and_two_are_never_attested() {
        let directory = std::env::temp_dir().join(format!("e2e-threads-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("harness");
        std::fs::copy("/bin/bash", &executable).unwrap();
        // A supervisor that starts one Harness from a thread that stays alive
        // (so it stays the child's parent), and with `main` one more from its
        // main thread.
        let supervise = |main: bool| {
            std::process::Command::new("python3")
                .args([
                    "-c",
                    "import subprocess, sys, threading, time\n\
                     start = lambda: subprocess.Popen([sys.argv[1], '-c', 'sleep 30; :'])\n\
                     threading.Thread(target=lambda: (start(), time.sleep(30)), daemon=True).start()\n\
                     if sys.argv[2] == 'main':\n    start()\n\
                     time.sleep(30)\n",
                ])
                .arg(&executable)
                .arg(if main { "main" } else { "thread" })
                .spawn()
                .unwrap()
        };
        let observe = |supervisor: &std::process::Child| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let observed = capture_compose(u64::from(supervisor.id()));
                if observed.is_ok() || std::time::Instant::now() > deadline {
                    break observed;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let mut one = supervise(false);
        let observed = observe(&one).unwrap();
        assert_ne!(observed.pid, u64::from(one.id()));
        let mut two = supervise(true);
        // Give both a moment to start: a refusal must not be a race.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let refused = observe(&two).unwrap_err();
        assert!(refused.to_string().contains("exactly one"), "{refused:#}");
        for supervisor in [&mut one, &mut two] {
            let _ = std::process::Command::new("pkill")
                .args(["-KILL", "-P", &supervisor.id().to_string()])
                .status();
            let _ = supervisor.kill();
            let _ = supervisor.wait();
        }
        std::fs::remove_file(executable).unwrap();
        std::fs::remove_dir(directory).unwrap();
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
            match capture_compose(u64::from(wrapper.id())) {
                Ok(observed) => break observed,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("{error:#}"),
            }
        };
        assert_ne!(observed.pid, u64::from(wrapper.id()));
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
