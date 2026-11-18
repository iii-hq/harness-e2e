//! The calls: find the stack's one judge provider, load its model, and send
//! it every case of a slice, one evaluation per request.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::cases::{self, Slice};
use super::score::{self, Evidence};
use crate::context::E2eContext;

/// The local providers, each served by a `judge-<provider>` worker.
pub const PROVIDERS: [&str; 4] = ["laya", "semif", "decider", "clef"];
/// The providers' `max_timeout_ms`.
const CASE_TIMEOUT_MS: u64 = 300_000;
const CALL_TIMEOUT: Duration = Duration::from_secs(310);
/// A cold model load, from a warm Hugging Face cache.
const WARMUP_WAIT: Duration = Duration::from_secs(600);
const WARMUP_RETRY: Duration = Duration::from_secs(5);

fn namespace(context: &E2eContext) -> String {
    context
        .client()
        .namespace()
        .unwrap_or_else(|| "default".to_string())
}

/// The stack's one judge provider and its worker's version. A Compose
/// worker and a hand-started one under the same name count as two.
pub async fn provider(context: &E2eContext) -> Result<(&'static str, String)> {
    let namespace = namespace(context);
    let listed = context
        .trigger_value("engine::workers::list", json!({}))
        .await
        .context("list the stack's workers")?;
    let found: Vec<(&'static str, String)> = listed["workers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|worker| worker["namespace"] == namespace.as_str())
        .filter_map(|worker| {
            let name = worker["name"].as_str()?;
            let provider = PROVIDERS
                .iter()
                .find(|provider| name == format!("judge-{provider}"))?;
            let version = worker["version"].as_str().unwrap_or_default().to_string();
            Some((*provider, version))
        })
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => bail!(
            "start a stack with exactly one of judge-laya, judge-semif, judge-decider, judge-clef (found: {})",
            if found.is_empty() {
                "none".to_string()
            } else {
                found
                    .iter()
                    .map(|(provider, _)| format!("judge-{provider}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
    }
}

/// Loads the provider's model with one tiny evaluation: `deadline` means the
/// load goes on, any other error is final.
pub async fn warm_up(context: &E2eContext, provider: &str) -> Result<()> {
    let function = format!("judge-{provider}::evaluate");
    let payload = json!({
        "timeout_ms": CASE_TIMEOUT_MS,
        "evaluations": [{
            "id": "warm-up",
            "state": "The sky is blue.",
            "questions": {"q": {"type": "noul", "instructions": "The text names a colour."}},
        }],
    });
    let started = Instant::now();
    loop {
        let reply = context
            .trigger_value_with_timeout(&function, payload.clone(), CALL_TIMEOUT)
            .await
            .with_context(|| format!("warm up {function}"))?;
        if reply["status"] == "ok" {
            return Ok(());
        }
        if reply["code"] != "deadline" {
            bail!("{function} cannot evaluate: {reply}");
        }
        if started.elapsed() >= WARMUP_WAIT {
            bail!(
                "{function} did not load its model within {} s: warm the Hugging Face cache first",
                WARMUP_WAIT.as_secs()
            );
        }
        tokio::time::sleep(WARMUP_RETRY).await;
    }
}

/// Every case of `slice`, one request each, with the provider's stored
/// configuration and model cards.
pub async fn measure(context: &E2eContext, slice: &Slice) -> Result<Evidence> {
    let (provider, version) = provider(context).await?;
    let worker = format!("judge-{provider}");
    let config_id = format!("{}-{worker}", namespace(context));
    let config = context
        .trigger_value("configuration::get", json!({"id": config_id}))
        .await
        .with_context(|| {
            format!(
                "read configuration {config_id}: start the worker with III_CONFIG_NAME={config_id}"
            )
        })?
        .get("value")
        .cloned()
        .unwrap_or(Value::Null);
    let cards = context
        .trigger_value_with_timeout(
            &format!("{worker}::models::list"),
            json!({"timeout_ms": CASE_TIMEOUT_MS}),
            CALL_TIMEOUT,
        )
        .await
        .with_context(|| format!("list {worker}'s models"))?
        .get("models")
        .cloned()
        .unwrap_or(Value::Null);
    let function = format!("{worker}::evaluate");
    let cases = cases::load(slice)?;
    let mut records = Vec::with_capacity(cases.len());
    let mut decisions = Vec::with_capacity(slice.decisions);
    for case in &cases {
        let payload = json!({
            "timeout_ms": CASE_TIMEOUT_MS,
            "evaluations": [{"id": case.id, "state": case.state, "questions": case.questions}],
        });
        let started = Instant::now();
        let reply = context
            .trigger_value_with_timeout(&function, payload, CALL_TIMEOUT)
            .await
            .with_context(|| format!("evaluate case {}", case.id))?;
        let ms = started.elapsed().as_secs_f64() * 1_000.0;
        let (record, decided) = score::decide(case, &reply, ms);
        records.push(record);
        decisions.extend(decided);
    }
    Ok(Evidence {
        provider: provider.to_string(),
        version,
        config,
        cards,
        ..Evidence::new(slice, &cases, records, decisions)
    })
}
