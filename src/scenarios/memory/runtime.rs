//! Plants, observes and removes what one memory attempt needs from the stack:
//! an isolated bank with the case's rules and memories, an observer on every
//! prompt Harness is about to send, a forced compaction, and the cleanup of
//! all three. Every `memory::*` call names the attempt's bank: left out, memory
//! would act on the user's own `main`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};
use tokio::time::sleep;

use super::cases::{bank, planted_memories, render, Case};
use super::score::PromptRecord;
use crate::context::E2eContext;
use crate::scenarios::common::lock_unpoisoned;

const PRE_GENERATE: &str = "harness::hook::pre-generate";
const EMBEDDER: &str = "all-MiniLM-L6-v2";
const POLL: Duration = Duration::from_secs(2);
const EMBEDDER_WAIT: Duration = Duration::from_secs(300);
const VECTOR_WAIT: Duration = Duration::from_secs(180);

/// What one attempt holds between `setup` and `cleanup`.
struct Run {
    prompts: Arc<Mutex<Vec<PromptRecord>>>,
    compaction: Option<String>,
    function: Option<FunctionRef>,
    trigger: Option<Trigger>,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The detail of the doctor's `embeddings-available` check, when it passes.
pub fn embedder(doctor: &Value) -> Option<&str> {
    doctor["checks"]
        .as_array()?
        .iter()
        .find(|check| check["name"] == "embeddings-available" && check["ok"] == true)?["detail"]
        .as_str()
}

/// Stores the prompt of a step of the attempt's own session. A pre-generate
/// hook has no session filter, so every session on the stack reaches this one.
pub fn record(sink: &Mutex<Vec<PromptRecord>>, session_id: &str, input: &Value) -> Value {
    if input["session_id"] == session_id {
        lock_unpoisoned(sink).push(PromptRecord {
            turn_id: input["turn_id"].as_str().unwrap_or_default().to_string(),
            step: input["step"].as_u64().unwrap_or_default(),
            system_prompt: input["generate"]["system_prompt"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        });
    }
    json!({"decision": "continue"})
}

/// A bank that was never created, or is already in the trash.
pub fn bank_already_gone(error: &str) -> bool {
    error.contains("bank_not_found")
}

/// Waits until memory embeds with MiniLM, the only model whose vectors the
/// cases are calibrated for.
async fn wait_for_embedder(context: &E2eContext) -> Result<()> {
    let deadline = Instant::now() + EMBEDDER_WAIT;
    loop {
        // Memory may not be registered yet; an error is the same as no answer.
        if let Ok(doctor) = context.trigger_value("memory::doctor", json!({})).await {
            match embedder(&doctor) {
                Some(detail) if detail.contains(EMBEDDER) => return Ok(()),
                Some(detail) => {
                    bail!("memory embeds with {detail}, not MiniLM: an embed key reached the stack")
                }
                None => {}
            }
        }
        if Instant::now() >= deadline {
            bail!("memory's embedder never answered");
        }
        sleep(POLL).await;
    }
}

/// Saves `text` and returns the id memory gave it.
async fn save(context: &E2eContext, bank: &str, text: &str) -> Result<String> {
    let saved = context
        .trigger_value("memory::save", json!({"bank": bank, "text": text}))
        .await
        .context("memory::save")?;
    match saved["memory"]["id"].as_str() {
        Some(id) => Ok(id.to_string()),
        None => bail!("memory::save returned no memory id: {saved}"),
    }
}

/// Waits until a semantic recall of the memory's own text returns it, which
/// means its vector is stored and a recall by meaning can find it.
async fn wait_for_vector(context: &E2eContext, bank: &str, id: &str, text: &str) -> Result<()> {
    let deadline = Instant::now() + VECTOR_WAIT;
    loop {
        let recalled = context
            .trigger_value(
                "memory::recall",
                json!({"bank": bank, "query": text, "mode": "semantic", "limit": 100}),
            )
            .await
            .context("memory::recall")?;
        let found = recalled["memories"]
            .as_array()
            .is_some_and(|hits| hits.iter().any(|hit| hit["memory"]["id"] == id));
        if found {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "memory never returned {id} for its own text (retrieval {})",
                recalled["retrieval"]
            );
        }
        sleep(POLL).await;
    }
}

/// Plants the case in its own bank and starts observing the attempt's prompts.
pub async fn setup(context: &E2eContext, case: &Case, run_id: &str) -> Result<()> {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    // Registered first, so `cleanup` undoes whatever a failed setup left.
    lock_unpoisoned(runs()).insert(
        run_id.to_string(),
        Run {
            prompts: prompts.clone(),
            compaction: None,
            function: None,
            trigger: None,
        },
    );

    wait_for_embedder(context).await?;
    let bank = bank(run_id);
    context
        .trigger_value("memory::bank::create", json!({"name": bank}))
        .await
        .context("memory::bank::create")?;

    if !case.baseline {
        // Rules first: memory freezes the system-prompt section at a session's first step.
        for rule in case.rules {
            context
                .trigger_value(
                    "memory::rule::set",
                    json!({"bank": bank, "name": rule.name, "content": render(rule.content, run_id)}),
                )
                .await
                .context("memory::rule::set")?;
        }
        let mut saved = Vec::new();
        for text in planted_memories(case, run_id) {
            saved.push((save(context, &bank, &text).await?, text));
        }
        for (id, text) in &saved {
            wait_for_vector(context, &bank, id, text).await?;
        }
    }

    let function_id = format!("e2e::memory::observe::{run_id}");
    let session_id = format!("e2e_{run_id}");
    let function = context.client().register_function(
        function_id.clone(),
        RegisterFunction::new_async(move |input: Value| {
            let prompts = prompts.clone();
            let session_id = session_id.clone();
            async move { Ok::<Value, iii_sdk::errors::Error>(record(&prompts, &session_id, &input)) }
        })
        .description("E2E memory group: records the system prompt of the attempt's own session.")
        .metadata(json!({"internal": true})),
    );
    lock_unpoisoned(runs())
        .get_mut(run_id)
        .expect("registered above")
        .function = Some(function);
    // Last in the chain, and open on error: it never changes or stops a turn.
    let trigger = context
        .client()
        .register_trigger(RegisterTriggerInput::new(
            PRE_GENERATE,
            function_id,
            json!({"priority": 1000, "timeout_ms": 2000, "on_error": "fail_open"}),
        ))?;
    lock_unpoisoned(runs())
        .get_mut(run_id)
        .expect("registered above")
        .trigger = Some(trigger);
    Ok(())
}

/// Compacts the session and keeps the outcome's status for `snapshot`.
pub async fn compact(context: &E2eContext, run_id: &str, session_id: &str) {
    let status = match context
        .trigger_value("harness::compact", json!({"session_id": session_id}))
        .await
    {
        Ok(outcome) => outcome["status"].as_str().map_or_else(
            || "error: reply without a status".to_string(),
            str::to_string,
        ),
        Err(error) => format!("error: {error}"),
    };
    if let Some(run) = lock_unpoisoned(runs()).get_mut(run_id) {
        run.compaction = Some(status);
    }
}

/// The prompts the observer recorded and the compaction status, if any.
pub fn snapshot(run_id: &str) -> (Vec<PromptRecord>, Option<String>) {
    lock_unpoisoned(runs()).get(run_id).map_or_else(
        || (Vec::new(), None),
        |run| {
            (
                lock_unpoisoned(&run.prompts).clone(),
                run.compaction.clone(),
            )
        },
    )
}

/// Unbinds the observer and trashes the attempt's bank.
pub async fn cleanup(context: &E2eContext, run_id: &str) -> Result<()> {
    let run = lock_unpoisoned(runs()).remove(run_id);
    if let Some(run) = run {
        if let Some(trigger) = run.trigger {
            trigger.unregister();
        }
        if let Some(function) = run.function {
            function.unregister();
        }
    }
    match context
        .trigger_value("memory::bank::delete", json!({"name": bank(run_id)}))
        .await
    {
        Err(error) if !bank_already_gone(&format!("{error:#}")) => {
            Err(error).context("memory::bank::delete")
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(session: &str, step: u64, prompt: &str) -> Value {
        json!({
            "point": "pre_generate",
            "session_id": session,
            "turn_id": "t_1",
            "step": step,
            "generate": {"system_prompt": prompt, "messages": []},
        })
    }

    #[test]
    fn observer_records_only_the_attempts_own_session() {
        let sink = Mutex::new(Vec::new());
        let own = record(&sink, "e2e_a", &input("e2e_a", 0, "prompt a"));
        let other = record(&sink, "e2e_a", &input("e2e_b", 0, "prompt b"));
        assert_eq!(own, json!({"decision": "continue"}));
        assert_eq!(other, json!({"decision": "continue"}));
        assert_eq!(
            *sink.lock().unwrap(),
            vec![PromptRecord {
                turn_id: "t_1".into(),
                step: 0,
                system_prompt: "prompt a".into(),
            }]
        );
    }

    #[test]
    fn the_embedder_is_read_from_the_doctor() {
        let doctor = |ok: bool| {
            json!({"checks": [
                {"name": "store-roundtrip", "ok": true, "detail": "ok"},
                {"name": "embeddings-available", "ok": ok, "detail": "router::embed answered (model sentence-transformers/all-MiniLM-L6-v2@c9745ed1d9f2)"},
            ]})
        };
        assert!(embedder(&doctor(true))
            .unwrap()
            .contains("all-MiniLM-L6-v2"));
        assert_eq!(embedder(&doctor(false)), None);
        assert_eq!(embedder(&json!({"checks": []})), None);
    }

    #[test]
    fn cleanup_ignores_a_bank_that_was_never_created() {
        assert!(bank_already_gone("bank_not_found: e2e-x"));
        assert!(!bank_already_gone("storage: disk full"));
    }
}
