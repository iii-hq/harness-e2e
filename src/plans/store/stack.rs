//! The stack a local execution runs on, recorded before its first slot: what
//! this worker's compose project asks for, what the engine reports running,
//! and the commit of every `path://` checkout. Whatever cannot be read becomes
//! a warning on the execution, never an error.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use serde_json::{json, Value};
use tokio::process::Command;

use super::{StackWorker, WorkerSource};
use crate::manifest::WORKER_NAME;

/// Ask the engine and compose what this worker's stack is.
pub(super) async fn observe(client: &iii_sdk::IIIClient) -> (Vec<StackWorker>, Vec<String>) {
    let namespace = client.namespace().unwrap_or_else(|| "default".into());
    let call = |function_id: &'static str, namespace: String| async move {
        client
            .trigger(
                TriggerRequest {
                    function_id: function_id.into(),
                    payload: json!({}),
                    action: None,
                    timeout_ms: Some(15_000),
                }
                .namespace(namespace),
            )
            .await
            .with_context(|| format!("invoke {function_id}"))
    };
    // `engine::workers::list` exists only in `default`; compose answers in
    // the namespace of the project it runs.
    let workers = call("engine::workers::list", "default".into()).await;
    let compose = call("compose::list", namespace.clone()).await;
    current(&namespace, workers, compose).await
}

/// The stack from what `engine::workers::list` and `compose::list` answered:
/// one row per container of the compose project that runs this worker, then
/// every other worker running in `namespace`.
pub(super) async fn current(
    namespace: &str,
    workers: Result<Value>,
    compose: Result<Value>,
) -> (Vec<StackWorker>, Vec<String>) {
    let mut warnings = Vec::new();
    let observed = match workers {
        Ok(workers) => observed_versions(&workers, Some(namespace)),
        Err(error) => {
            warnings.push(format!(
                "Running worker versions were not recorded: {error:#}"
            ));
            BTreeMap::new()
        }
    };
    let (containers, directory) = async {
        let compose = compose?;
        let file = compose["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|project| {
                project["containers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|container| container["container"] == WORKER_NAME)
            })
            .and_then(|project| project["file"].as_str())
            .with_context(|| {
                format!("no compose project in namespace {namespace} runs {WORKER_NAME}")
            })?
            .to_owned();
        let source = std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?;
        let document: Value =
            serde_yaml::from_str(&source).with_context(|| format!("decode {file}"))?;
        // Compose reads a relative `path://` from the file's directory.
        let directory = Path::new(&file).parent().map(Path::to_path_buf);
        Ok::<_, anyhow::Error>((
            document["containers"].clone(),
            directory.unwrap_or_default(),
        ))
    }
    .await
    .unwrap_or_else(|error| {
        warnings.push(format!("Worker sources were not recorded: {error:#}"));
        (Value::Null, PathBuf::new())
    });
    let mut stack = rows(
        &containers,
        |container| container["version"].as_str().map(str::to_owned),
        observed,
    );
    for row in stack
        .iter_mut()
        .filter(|row| row.source == WorkerSource::Path)
    {
        let Some(path) = containers[row.name.as_str()]["worker"]
            .as_str()
            .and_then(|worker| worker.strip_prefix("path://"))
        else {
            continue;
        };
        match checkout(&directory.join(path)).await {
            Ok((commit, dirty)) => {
                row.commit = Some(commit);
                row.dirty = Some(dirty);
            }
            Err(error) => warnings.push(format!("{}: {error:#}", row.name)),
        }
    }
    stack.sort_by(|left, right| left.name.cmp(&right.name));
    (stack, warnings)
}

/// Name and version of each running worker, engine built-ins left out; with
/// a namespace, only the workers registered in it.
pub(super) fn observed_versions(
    workers: &Value,
    namespace: Option<&str>,
) -> BTreeMap<String, Option<String>> {
    workers["workers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|worker| worker["runtime"] != "engine")
        .filter(|worker| !worker["name"].as_str().is_some_and(cli_client))
        .filter(|worker| namespace.is_none_or(|namespace| worker["namespace"] == namespace))
        .filter_map(|worker| {
            Some((
                worker["name"].as_str()?.to_owned(),
                worker["version"].as_str().map(str::to_owned),
            ))
        })
        .collect()
}

/// `iii trigger` connects as `<host>:<pid>` for the length of one call; it is
/// the CLI, not a worker of the stack.
fn cli_client(name: &str) -> bool {
    name.rsplit_once(':').is_some_and(|(host, pid)| {
        !host.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
    })
}

/// One row per container (of a compose file or lock) with the version the
/// engine reports for it, then the running workers no container declares.
pub(super) fn rows(
    containers: &Value,
    requested: impl Fn(&Value) -> Option<String>,
    mut observed: BTreeMap<String, Option<String>>,
) -> Vec<StackWorker> {
    let mut workers = Vec::new();
    for (name, container) in containers.as_object().into_iter().flatten() {
        workers.push(StackWorker {
            name: name.clone(),
            source: if container["worker"]
                .as_str()
                .is_some_and(|worker| worker.starts_with("path:"))
            {
                WorkerSource::Path
            } else {
                WorkerSource::Package
            },
            requested: requested(container),
            observed: observed.remove(name).flatten(),
            commit: None,
            dirty: None,
            groups: Vec::new(),
        });
    }
    workers.extend(observed.into_iter().map(|(name, observed)| StackWorker {
        name,
        source: WorkerSource::Package,
        requested: None,
        observed,
        commit: None,
        dirty: None,
        groups: Vec::new(),
    }));
    workers
}

/// The commit of the repository a worker's path is in, and whether that
/// path (not the rest of the repository) has local changes.
async fn checkout(path: &Path) -> Result<(String, bool)> {
    let commit = git(path, &["rev-parse", "HEAD"]).await?;
    // Read-only: no `index.lock` taken while a developer works in the tree.
    let status = git(
        path,
        &["--no-optional-locks", "status", "--porcelain", "--", "."],
    )
    .await?;
    Ok((commit.trim().to_owned(), !status.trim().is_empty()))
}

async fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .with_context(|| {
        format!(
            "`git {}` in {} took too long",
            args.join(" "),
            path.display()
        )
    })?
    .with_context(|| format!("run git in {}", path.display()))?;
    ensure!(
        output.status.success(),
        "{} is not a readable Git checkout: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(directory: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["-c", "user.name=e2e", "-c", "user.email=e2e@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn compose_list(file: &Path) -> Value {
        json!({"daemon": "compose", "projects": [
            {"file": "/elsewhere/worker-compose.yaml", "namespace": "my-project",
             "containers": [{"container": "other", "state": "ready"}]},
            {"file": file, "namespace": "my-project",
             "containers": [{"container": "queue"}, {"container": WORKER_NAME}]},
        ]})
    }

    #[test]
    fn cli_connections_are_not_stack_workers() {
        let workers = json!({"workers": [
            {"name": "harness", "version": "1.8.8", "runtime": "rust"},
            {"name": "runnervmlun5p:2889", "version": "0.24.2", "runtime": "rust"},
            {"name": "iii-http", "version": "0.24.2", "runtime": "engine"},
        ]});
        let observed = observed_versions(&workers, None);
        assert_eq!(observed.keys().collect::<Vec<_>>(), ["harness"]);
    }

    #[tokio::test]
    async fn records_package_and_path_workers_with_the_checkout_commit() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("workers");
        for worker in ["queue", "state"] {
            std::fs::create_dir_all(checkout.join(worker)).unwrap();
            std::fs::write(checkout.join(worker).join("lib.rs"), "fn main() {}\n").unwrap();
        }
        run_git(&checkout, &["init", "--quiet"]);
        run_git(&checkout, &["add", "."]);
        run_git(&checkout, &["commit", "--quiet", "-m", "workers"]);
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
        // Only queue changes; a change elsewhere in the repository is no
        // worker's.
        std::fs::write(checkout.join("queue/lib.rs"), "fn main() { todo() }\n").unwrap();
        std::fs::write(checkout.join("README.md"), "notes\n").unwrap();
        let compose = root.path().join("worker-compose.yaml");
        // `state` is relative: compose reads it from the file's directory.
        std::fs::write(
            &compose,
            format!(
                "namespace: my-project\ncontainers:\n  queue:\n    worker: path://{}\n  state:\n    worker: path://workers/state\n  storage:\n    worker: package://api.workers.iii.dev/storage\n    version: \"0.1.20\"\n  {WORKER_NAME}:\n    worker: path://{}\n",
                checkout.join("queue").display(),
                root.path().join("missing").display(),
            ),
        )
        .unwrap();
        let workers = json!({"workers": [
            {"name": "queue", "version": "0.4.1", "namespace": "my-project", "runtime": "rust"},
            {"name": "storage", "version": "0.1.19", "namespace": "my-project", "runtime": "rust"},
            {"name": "harness", "version": "1.8.8", "namespace": "my-project", "runtime": "rust"},
            {"name": "harness", "version": "9.9.9", "namespace": "default", "runtime": "rust"},
            {"name": "configuration", "version": "0.24.0", "namespace": "my-project", "runtime": "engine"},
        ]});

        let (stack, warnings) =
            current("my-project", Ok(workers), Ok(compose_list(&compose))).await;

        let row = |name: &str| stack.iter().find(|row| row.name == name).unwrap();
        assert_eq!(
            stack
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["harness", WORKER_NAME, "queue", "state", "storage"]
        );
        let queue = row("queue");
        assert_eq!(queue.source, WorkerSource::Path);
        assert_eq!(queue.observed.as_deref(), Some("0.4.1"));
        assert_eq!(queue.commit.as_deref(), Some(head.as_str()));
        assert_eq!(queue.dirty, Some(true));
        // Same repository and commit, no change under its own path.
        let state = row("state");
        assert_eq!(state.source, WorkerSource::Path);
        assert_eq!(state.commit.as_deref(), Some(head.as_str()));
        assert_eq!(state.dirty, Some(false));
        let storage = row("storage");
        assert_eq!(storage.source, WorkerSource::Package);
        assert_eq!(
            (storage.requested.as_deref(), storage.observed.as_deref()),
            (Some("0.1.20"), Some("0.1.19"))
        );
        assert_eq!((storage.commit.as_deref(), storage.dirty), (None, None));
        // Running but declared by no container: the engine's version only.
        let harness = row("harness");
        assert_eq!(
            (harness.requested.as_deref(), harness.observed.as_deref()),
            (None, Some("1.8.8"))
        );
        // A path that cannot be read keeps its row and becomes a warning.
        assert_eq!(row(WORKER_NAME).commit, None);
        let [warning] = warnings.as_slice() else {
            panic!("one warning: {warnings:?}");
        };
        assert!(
            warning.starts_with(&format!("{WORKER_NAME}: ")) && warning.contains("Git checkout"),
            "{warning}"
        );
        // Reading the status took no lock a developer's git could trip on.
        assert!(!checkout.join(".git/index.lock").exists());

        // A clean worker path is not dirty.
        run_git(&checkout, &["checkout", "--quiet", "--", "queue"]);
        let (stack, _) = current(
            "my-project",
            Ok(json!({"workers": []})),
            Ok(compose_list(&compose)),
        )
        .await;
        assert_eq!(
            stack.iter().find(|row| row.name == "queue").unwrap().dirty,
            Some(false)
        );
    }

    #[tokio::test]
    async fn without_compose_the_running_workers_are_kept_with_a_warning() {
        let workers = json!({"workers": [
            {"name": "harness", "version": "1.8.8", "namespace": "ns", "runtime": "rust"},
        ]});
        let (stack, warnings) = current(
            "ns",
            Ok(workers),
            Err(anyhow::anyhow!("function_not_found: compose::list")),
        )
        .await;
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].observed.as_deref(), Some("1.8.8"));
        assert_eq!(stack[0].requested, None);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("compose::list"), "{warnings:?}");

        // Compose answers but runs no project with this worker; the engine
        // cannot be asked either. Nothing is recorded, nothing fails.
        let (stack, warnings) = current(
            "ns",
            Err(anyhow::anyhow!("engine unavailable")),
            Ok(json!({"projects": []})),
        )
        .await;
        assert!(stack.is_empty());
        assert_eq!(warnings.len(), 2);
        assert!(warnings[1].contains("no compose project in namespace ns"));
    }
}
