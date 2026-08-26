use std::collections::BTreeSet;
use std::fs::{self, File};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::process::{Child, Command};

use crate::context::E2eContext;

use super::{ImprovementLoopSpecV1, ImprovementModel};

pub(super) struct EvaluationStack {
    url: String,
    children: Vec<(String, Child)>,
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl EvaluationStack {
    pub(super) async fn start(
        spec: &ImprovementLoopSpecV1,
        source_root: &Path,
        harness_bin: &Path,
        run_dir: &Path,
    ) -> Result<Self> {
        spec.verify_stack_binaries()?;
        let port = reserve_port(spec.stack.preferred_port)?;
        let url = format!("ws://127.0.0.1:{port}");
        let logs_dir = run_dir.join("logs");
        let data_dir = run_dir.join("stack");
        fs::create_dir_all(&logs_dir).with_context(|| format!("create {}", logs_dir.display()))?;
        fs::create_dir_all(data_dir.join("database"))
            .with_context(|| format!("create {}", data_dir.display()))?;
        let config_root = source_root.join("harness/tests/e2e/stack-config");
        let engine_template = fs::read_to_string(config_root.join("engine.yaml"))
            .context("read pinned E2E engine config")?;
        let engine_config = data_dir.join("config.yaml");
        fs::write(
            &engine_config,
            engine_template.replace("${HARNESS_E2E_PORT}", &port.to_string()),
        )
        .with_context(|| format!("write {}", engine_config.display()))?;
        let mut stack = Self {
            url: url.clone(),
            children: Vec::new(),
            #[cfg(unix)]
            process_group: None,
        };
        stack.spawn(
            "engine",
            &spec.stack.iii_bin,
            &["-c".into(), path_arg(&engine_config)],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        let context = E2eContext::connect(&url)
            .await
            .context("connect to isolated evaluation engine")?;
        wait_for_function(&context, "engine::workers::list").await?;

        let binary_root = &spec.stack.workers_binary_root;
        stack.spawn_worker(
            "database",
            &binary_root.join("database/target/release/database"),
            &[
                "--config".into(),
                path_arg(&config_root.join("database.yaml")),
            ],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "database::query").await?;
        stack.spawn_worker(
            "state",
            &binary_root.join("state/target/release/state"),
            &["--config".into(), path_arg(&config_root.join("state.yaml"))],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "state::get").await?;
        stack.spawn_worker(
            "queue",
            &binary_root.join("queue/target/release/queue"),
            &["--config".into(), path_arg(&config_root.join("queue.yaml"))],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "queue::define").await?;
        stack.spawn_worker(
            "fp",
            &binary_root.join("fp/target/release/fp"),
            &[],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "fp::when").await?;
        stack.spawn_worker(
            "session-manager",
            &binary_root.join("session-manager/target/release/session-manager"),
            &[
                "--config".into(),
                path_arg(&config_root.join("session-manager.yaml")),
            ],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "session::messages").await?;
        stack.spawn_worker(
            "llm-router",
            &binary_root.join("llm-router/target/release/llm-router"),
            &[],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "router::models::get").await?;

        let models = [&spec.subject, &spec.judge];
        let mut providers = BTreeSet::new();
        for model in models {
            if providers.insert(model.provider.clone()) {
                validate_provider(&model.provider)?;
                let worker = format!("provider-{}", model.provider);
                stack.spawn_worker(
                    &worker,
                    &binary_root.join(format!("{worker}/target/release/{worker}")),
                    &[],
                    &data_dir,
                    &logs_dir,
                    &url,
                )?;
                wait_for_function(&context, &format!("provider::{}::stream", model.provider))
                    .await?;
            }
        }
        for model in models {
            wait_for_model(&context, model).await?;
        }

        stack.spawn_worker(
            "context-manager",
            &binary_root.join("context-manager/target/release/context-manager"),
            &[
                "--config".into(),
                path_arg(&config_root.join("context-manager.yaml")),
            ],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "context::assemble").await?;
        stack.spawn_worker(
            "iii-directory",
            &binary_root.join("iii-directory/target/release/iii-directory"),
            &[],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        wait_for_function(&context, "directory::skills::list").await?;
        stack.spawn_worker(
            "cron",
            &binary_root.join("cron/target/release/cron"),
            &[],
            &data_dir,
            &logs_dir,
            &url,
        )?;
        stack.spawn_worker("harness", harness_bin, &[], &data_dir, &logs_dir, &url)?;
        wait_for_function(&context, "harness::send").await?;
        context.shutdown().await;
        Ok(stack)
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }

    pub(super) async fn shutdown(mut self) {
        self.signal_process_group(libc::SIGTERM);
        for (_, mut child) in self.children.drain(..).rev() {
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                #[cfg(not(unix))]
                let _ = child.start_kill();
            }
        }
        self.signal_process_group(libc::SIGKILL);
    }

    fn spawn_worker(
        &mut self,
        name: &str,
        binary: &Path,
        extra_args: &[String],
        data_dir: &Path,
        logs_dir: &Path,
        url: &str,
    ) -> Result<()> {
        let mut args = vec!["--url".into(), url.into()];
        args.extend_from_slice(extra_args);
        self.spawn(name, binary, &args, data_dir, logs_dir, url)
    }

    fn spawn(
        &mut self,
        name: &str,
        binary: &Path,
        args: &[String],
        data_dir: &Path,
        logs_dir: &Path,
        url: &str,
    ) -> Result<()> {
        if !binary.is_file() {
            bail!(
                "required evaluation binary is missing: {}",
                binary.display()
            );
        }
        let stdout = File::create(logs_dir.join(format!("{name}.log")))?;
        let stderr = stdout.try_clone()?;
        let mut command = Command::new(binary);
        command
            .args(args)
            .current_dir(data_dir)
            .env("III_ENGINE_URL", url)
            .env("HARNESS_E2E_RUN_DIR", data_dir)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(self.process_group.unwrap_or(0));
        let child = command
            .spawn()
            .with_context(|| format!("start evaluation process {name}"))?;
        #[cfg(unix)]
        if self.process_group.is_none() {
            self.process_group = child.id().and_then(|id| i32::try_from(id).ok());
        }
        self.children.push((name.into(), child));
        Ok(())
    }

    fn signal_process_group(&self, signal: i32) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group {
            // SAFETY: the supervisor created this process group and retains its child handles
            // until shutdown, so the negative PID is scoped to the isolated evaluation stack.
            unsafe {
                libc::kill(-process_group, signal);
            }
        }
        #[cfg(not(unix))]
        let _ = signal;
    }
}

impl Drop for EvaluationStack {
    fn drop(&mut self) {
        self.signal_process_group(libc::SIGKILL);
        for (_, child) in self.children.iter_mut().rev() {
            let _ = child.start_kill();
        }
    }
}

fn reserve_port(preferred: Option<u16>) -> Result<u16> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, preferred.unwrap_or(0));
    let listener = TcpListener::bind(address)
        .with_context(|| format!("reserve isolated evaluation port {address}"))?;
    Ok(listener.local_addr()?.port())
}

async fn wait_for_function(context: &E2eContext, function_id: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if context.function_exists(function_id).await.unwrap_or(false) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("timed out waiting for evaluation function {function_id}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn wait_for_model(context: &E2eContext, model: &ImprovementModel) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let response = context
            .trigger_value(
                "router::models::get",
                json!({"provider": model.provider, "id": model.model}),
            )
            .await;
        if let Ok(response) = response {
            if model_id(response.clone()).as_deref() == Some(model.model.as_str()) {
                if response
                    .get("model")
                    .and_then(|model| model.get("pricing"))
                    .is_none_or(Value::is_null)
                {
                    bail!(
                        "evaluation model {}/{} has no catalog pricing for the mandatory cost budget",
                        model.provider,
                        model.model
                    );
                }
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timed out waiting for evaluation model {}/{}",
                model.provider,
                model.model
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn model_id(value: Value) -> Option<String> {
    value.get("model")?.get("id")?.as_str().map(str::to_string)
}

fn validate_provider(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("unsupported provider id '{value}'");
    }
    Ok(())
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
