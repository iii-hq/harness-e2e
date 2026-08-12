use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use harness_e2e::control::ControlPlane;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::runtime::WorkerMetadata;
use iii_sdk::{register_worker, InitOptions};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(
    name = "e2e-worker",
    about = "Expose the asynchronous Harness E2E control plane through iii."
)]
struct Cli {
    #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
    url: String,

    #[arg(
        long,
        env = "HARNESS_E2E_OUTPUT_ROOT",
        default_value = "target/e2e-worker"
    )]
    output_root: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let iii = register_worker(
        &cli.url,
        InitOptions {
            metadata: Some(WorkerMetadata {
                runtime: "rust".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                name: "e2e".into(),
                os: std::env::consts::OS.into(),
                pid: Some(std::process::id()),
                ..WorkerMetadata::default()
            }),
            ..InitOptions::default()
        },
    );
    wait_for_state(&iii).await?;
    let control = ControlPlane::new(iii.clone(), cli.url, cli.output_root)
        .await
        .context("restore the E2E control plane")?;
    control.register();
    tracing::info!("E2E worker ready: e2e::* control functions registered");
    tokio::signal::ctrl_c().await?;
    iii.shutdown_async().await;
    Ok(())
}

async fn wait_for_state(iii: &iii_sdk::IIIClient) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ready = iii
            .trigger(TriggerRequest {
                function_id: "engine::functions::list".into(),
                payload: json!({ "include_internal": true }),
                action: None,
                timeout_ms: Some(1_000),
            })
            .await
            .ok()
            .and_then(|value| serde_json::to_string(&value).ok())
            .is_some_and(|value| value.contains("state::list"));
        if ready {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("state::list was not ready within 30 seconds");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
