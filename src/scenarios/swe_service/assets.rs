use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;
use serde_json::Value;
use tokio::process::Command;

use super::FIXTURE_REVISION;

#[derive(RustEmbed)]
#[folder = "src/scenarios/swe_service/"]
#[include = "*.py"]
struct PythonAssets;

#[derive(RustEmbed)]
#[folder = "tests/fixtures/campaign/"]
#[include = "swe-service.bundle"]
struct FixtureAssets;

pub async fn unpack(root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    for name in ["controller.py", "probes.py", "isolation.py"] {
        let data = PythonAssets::get(name)
            .with_context(|| format!("missing embedded SWE asset {name}"))?;
        std::fs::write(root.join(name), data.data.as_ref())?;
    }
    let bundle = FixtureAssets::get("swe-service.bundle")
        .context("the executor has no immutable SWE fixture bundle")?;
    let bundle_path = root.join("swe-service.bundle");
    std::fs::write(&bundle_path, bundle.data.as_ref())?;
    let source = root.join("source");
    command(
        "git",
        &[
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "clone".into(),
            "--quiet".into(),
            "--no-checkout".into(),
            "--no-hardlinks".into(),
            bundle_path.to_string_lossy().into_owned(),
            source.to_string_lossy().into_owned(),
        ],
        Duration::from_secs(60),
    )
    .await?;
    command(
        "git",
        &[
            "-C".into(),
            source.to_string_lossy().into_owned(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "checkout".into(),
            "--quiet".into(),
            "--detach".into(),
            FIXTURE_REVISION.into(),
        ],
        Duration::from_secs(60),
    )
    .await?;
    let head = command(
        "git",
        &[
            "-C".into(),
            source.to_string_lossy().into_owned(),
            "rev-parse".into(),
            "HEAD".into(),
        ],
        Duration::from_secs(10),
    )
    .await?;
    if String::from_utf8_lossy(&head).trim() != FIXTURE_REVISION {
        bail!("SWE fixture revision does not match its immutable pin");
    }
    Ok(source)
}

pub async fn controller(root: &Path, args: &[String]) -> Result<Value> {
    let mut parameters = vec![
        "-I".into(),
        root.join("controller.py").to_string_lossy().into_owned(),
    ];
    parameters.extend(args.iter().cloned());
    let bytes = command("python3", &parameters, Duration::from_secs(245)).await?;
    serde_json::from_slice(&bytes).context("SWE controller returned invalid JSON")
}

pub async fn command(program: &str, args: &[String], timeout: Duration) -> Result<Vec<u8>> {
    #[cfg(not(unix))]
    bail!("SWE trusted commands require Unix process isolation");
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LANG", "C.UTF-8")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    // Backend selection is operator configuration, never supplied by a subject.
    for name in [
        "HARNESS_E2E_SWE_ISOLATION_BACKEND",
        "HARNESS_E2E_SWE_DOCKER_IMAGE",
        "DOCKER_HOST",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(unix)]
    command.process_group(0);
    let child = command
        .spawn()
        .with_context(|| format!("execute trusted SWE {program}"))?;
    let pid = child
        .id()
        .context("trusted SWE command has no process id")?;
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished_task = finished.clone();
    // Keep reaping even if the enclosing workflow future is cancelled.
    let mut output_task = tokio::spawn(async move {
        let output = child.wait_with_output().await;
        finished_task.store(true, std::sync::atomic::Ordering::Release);
        output
    });
    let mut guard = CommandGuard {
        pid,
        finished,
        armed: true,
    };
    let output = match tokio::time::timeout(timeout, &mut output_task).await {
        Ok(output) => {
            guard.armed = false;
            output.context("join trusted SWE command")??
        }
        Err(_) => {
            guard.terminate();
            // The controller handles TERM and propagates it to verifier sessions.
            let _ = tokio::time::timeout(Duration::from_secs(3), output_task).await;
            bail!("trusted SWE command exceeded its deadline");
        }
    };
    if output.stdout.len() > 64 * 1024 * 1024 || output.stderr.len() > 1024 * 1024 {
        bail!("trusted SWE command exceeded its evidence output limit");
    }
    if !output.status.success() {
        // Controller operational errors are deliberately bounded and omit probe internals.
        let detail = serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .or_else(|| value.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        bail!(
            "trusted SWE command failed ({}): {}",
            output.status,
            detail.unwrap_or_else(|| String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1_000)
                .collect())
        );
    }
    Ok(output.stdout)
}

struct CommandGuard {
    pid: u32,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
    armed: bool,
}

impl CommandGuard {
    fn terminate(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        if self.finished.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        signal_group(self.pid, 15);
        let pid = self.pid;
        let finished = self.finished.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if !finished.load(std::sync::atomic::Ordering::Acquire) {
                signal_group(pid, 9);
            }
        });
    }
}

impl Drop for CommandGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // Each trusted command is spawned in its own process group, never the runner's.
    unsafe {
        kill(-(pid as i32), signal);
    }
}

#[cfg(not(unix))]
fn signal_group(_pid: u32, _signal: i32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embedded_bundle_unpacks_exact_pin() {
        let temp = tempfile::tempdir().unwrap();
        let source = unpack(temp.path()).await.unwrap();
        let head = command(
            "git",
            &[
                "-C".into(),
                source.to_string_lossy().into_owned(),
                "rev-parse".into(),
                "HEAD".into(),
            ],
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(String::from_utf8(head).unwrap().trim(), FIXTURE_REVISION);
    }

    #[tokio::test]
    async fn command_timeout_and_future_drop_terminate_separate_verifier_sessions() {
        for cancelled in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let controller = temp.path().join("controller.py");
            std::fs::write(
                &controller,
                PythonAssets::get("controller.py").unwrap().data.as_ref(),
            )
            .unwrap();
            let ready = temp.path().join("ready");
            let stopped = temp.path().join("stopped");
            let wrapper = temp.path().join("wrapper.py");
            std::fs::write(&wrapper, r#"import importlib.util,pathlib,signal,sys
spec=importlib.util.spec_from_file_location('controller',sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
signal.signal(signal.SIGTERM,module.interrupted)
script="import pathlib,signal,sys,time; signal.signal(signal.SIGTERM,lambda *_:(pathlib.Path(sys.argv[2]).write_text('stopped'),sys.exit(0))); pathlib.Path(sys.argv[1]).write_text(str(__import__('os').getpid())); time.sleep(60)"
module.run([sys.executable,'-I','-c',script,sys.argv[2],sys.argv[3]])
"#).unwrap();
            let args = vec![
                "-I".into(),
                wrapper.to_string_lossy().into_owned(),
                controller.to_string_lossy().into_owned(),
                ready.to_string_lossy().into_owned(),
                stopped.to_string_lossy().into_owned(),
            ];
            let timeout = if cancelled {
                Duration::from_secs(60)
            } else {
                Duration::from_millis(500)
            };
            let task = tokio::spawn(async move { command("python3", &args, timeout).await });
            for _ in 0..100 {
                if ready.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(ready.exists(), "verifier should have started");
            let pid = std::fs::read_to_string(&ready).unwrap();
            if cancelled {
                task.abort();
                let _ = task.await;
            } else {
                assert!(task.await.unwrap().is_err());
            }
            for _ in 0..100 {
                if stopped.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                stopped.exists(),
                "TERM must reach a verifier in another session"
            );
            let status = std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "verifier must be reaped");
        }
    }
}
