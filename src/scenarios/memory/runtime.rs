//! Plants, observes and removes what one memory attempt needs from the stack:
//! an isolated bank with the case's rules and memories, an observer on every
//! prompt Harness is about to send, the case's changes between exchanges, a
//! forced compaction, and the cleanup of all of it. Every `memory::*` call
//! names the attempt's bank: left out, memory would act on the user's own
//! `main`.

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

use super::cases::{bank, planted, render, Case, Change};
use super::score::{ChangeOutcome, PromptRecord, Retrieval};
use crate::context::E2eContext;
use crate::scenarios::common::lock_unpoisoned;

const PRE_GENERATE: &str = "harness::hook::pre-generate";
const EMBEDDER: &str = "all-MiniLM-L6-v2";
const POLL: Duration = Duration::from_secs(2);
const EMBEDDER_WAIT: Duration = Duration::from_secs(300);
const VECTOR_WAIT: Duration = Duration::from_secs(180);
/// How long lexical-only must hold before it is believed.
const LEXICAL_SETTLE: Duration = Duration::from_secs(15);

/// What one attempt holds between `setup` and `cleanup`.
struct Run {
    prompts: Arc<Mutex<Vec<PromptRecord>>>,
    retrieval: Option<Retrieval>,
    compaction: Option<String>,
    function: Option<FunctionRef>,
    trigger: Option<Trigger>,
    /// Each case seed's memory once saved, parallel to the case's memories.
    seeds: Vec<Option<Saved>>,
    changes: Vec<ChangeOutcome>,
}

/// A memory as memory stored it.
#[derive(Debug, Clone, PartialEq)]
struct Saved {
    id: String,
    text: String,
}

/// What `capture` reads of an attempt.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub prompts: Vec<PromptRecord>,
    pub compaction: Option<String>,
    pub retrieval: Option<Retrieval>,
    /// Memory's stored text of each case seed, `None` until saved.
    pub stored: Vec<Option<String>>,
    pub changes: Vec<ChangeOutcome>,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What `memory::doctor` says about the embedder.
#[derive(Debug, PartialEq)]
pub enum Embedder {
    Ready(Retrieval),
    /// Another model embeds (an embed key reached the stack): the cases are
    /// calibrated for MiniLM vectors or none.
    Other(String),
    /// Memory, its router or its state is not up yet.
    Pending,
}

pub fn embedder(doctor: &Value) -> Embedder {
    let check = |name: &str| {
        doctor["checks"]
            .as_array()
            .and_then(|checks| checks.iter().find(|check| check["name"] == name))
    };
    let up = |name: &str| check(name).is_some_and(|check| check["ok"] == true);
    // Before the router answers, "no vector" says nothing about the embedder.
    let Some(embeddings) =
        check("embeddings-available").filter(|_| up("router-reachable") && up("state-reachable"))
    else {
        return Embedder::Pending;
    };
    let detail = embeddings["detail"].as_str().unwrap_or_default();
    if detail.contains("lexical-only") {
        Embedder::Ready(Retrieval::Lexical)
    } else if embeddings["ok"] != true {
        Embedder::Pending
    } else if detail.contains(EMBEDDER) {
        Embedder::Ready(Retrieval::Minilm)
    } else {
        Embedder::Other(detail.to_string())
    }
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

/// Whether lexical-only, first answered at `first`, has held long enough: a
/// stack whose MiniLM is still starting answers lexical-only too.
pub fn lexical_settled(first: Instant, now: Instant) -> bool {
    now.duration_since(first) >= LEXICAL_SETTLE
}

/// A bank that was never created, or is already in the trash.
pub fn bank_already_gone(error: &str) -> bool {
    error.contains("bank_not_found")
}

/// Waits until memory ranks with MiniLM or lexical-only, the two modes the
/// suite accepts, and says which. MiniLM is taken at once; lexical-only once
/// two answers at least `LEXICAL_SETTLE` apart both say so.
async fn wait_for_embedder(context: &E2eContext) -> Result<Retrieval> {
    let deadline = Instant::now() + EMBEDDER_WAIT;
    let mut first_lexical = None;
    loop {
        // Memory may not be registered yet; an error is the same as no answer.
        let last = match context.trigger_value("memory::doctor", json!({})).await {
            Ok(doctor) => match embedder(&doctor) {
                Embedder::Ready(Retrieval::Lexical) => {
                    let now = Instant::now();
                    if lexical_settled(*first_lexical.get_or_insert(now), now) {
                        return Ok(Retrieval::Lexical);
                    }
                    doctor.to_string()
                }
                Embedder::Ready(retrieval) => return Ok(retrieval),
                Embedder::Other(detail) => {
                    bail!("memory embeds with {detail}, not MiniLM: an embed key reached the stack")
                }
                Embedder::Pending => doctor.to_string(),
            },
            Err(error) => format!("{error:#}"),
        };
        if Instant::now() >= deadline {
            bail!("memory's embedder never settled; last doctor answer: {last}");
        }
        sleep(POLL).await;
    }
}

fn save_request(bank: &str, text: &str, entities: &[&str]) -> Value {
    json!({"bank": bank, "text": text, "entities": entities})
}

/// Saves `text` and returns the memory as memory stored it.
async fn save(context: &E2eContext, bank: &str, text: &str, entities: &[&str]) -> Result<Saved> {
    let reply = context
        .trigger_value("memory::save", save_request(bank, text, entities))
        .await
        .context("memory::save")?;
    created(&reply)
}

/// The memory a reply carries, as memory stored it.
fn stored(reply: &Value) -> Result<Saved> {
    match (
        reply["memory"]["id"].as_str(),
        reply["memory"]["text"].as_str(),
    ) {
        (Some(id), Some(text)) => Ok(Saved {
            id: id.to_string(),
            text: text.to_string(),
        }),
        _ => bail!("the reply carries no memory id and text: {reply}"),
    }
}

/// The memory a save created: one that reinforced an existing memory is not
/// the memory the case planned.
fn created(reply: &Value) -> Result<Saved> {
    if reply["created"] != true {
        bail!("memory::save reinforced an existing memory instead of creating one: {reply}");
    }
    stored(reply)
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
            retrieval: None,
            compaction: None,
            function: None,
            trigger: None,
            seeds: vec![None; case.memories.len()],
            changes: Vec::new(),
        },
    );

    let retrieval = wait_for_embedder(context).await?;
    lock_unpoisoned(runs())
        .get_mut(run_id)
        .expect("registered above")
        .retrieval = Some(retrieval);
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
        for (seed, template) in planted(case) {
            let entities = seed.map_or(&[][..], |index| case.memories[index].entities);
            let memory = save(context, &bank, &render(template, run_id), entities).await?;
            if let Some(index) = seed {
                keep(run_id, index, memory.clone());
            }
            saved.push(memory);
        }
        // Lexical-only stores no vector to wait for.
        if retrieval == Retrieval::Minilm {
            for memory in &saved {
                wait_for_vector(context, &bank, &memory.id, &memory.text).await?;
            }
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

/// Keeps seed `index`'s memory for the changes and the evidence.
fn keep(run_id: &str, index: usize, memory: Saved) {
    if let Some(slot) = lock_unpoisoned(runs())
        .get_mut(run_id)
        .and_then(|run| run.seeds.get_mut(index))
    {
        *slot = Some(memory);
    }
}

/// The payload of the memory call `change` makes; `ids` holds the memory id
/// of each case seed saved so far.
fn change_payload(
    case: &Case,
    change: Change,
    run_id: &str,
    ids: &[Option<String>],
) -> Result<Value> {
    let bank = bank(run_id);
    let id = |seed: usize| {
        ids.get(seed)
            .cloned()
            .flatten()
            .with_context(|| format!("seed {seed} has no memory: it was never saved"))
    };
    let text = |seed: usize| render(case.memories[seed].text, run_id);
    Ok(match change {
        Change::RuleSet { name, content } => {
            json!({"bank": bank, "name": name, "content": render(content, run_id)})
        }
        Change::Save { seed } => save_request(&bank, &text(seed), case.memories[seed].entities),
        Change::Delete { seed } => json!({"bank": bank, "id": id(seed)?}),
        Change::Supersede { old, new } => {
            json!({"bank": bank, "id": id(old)?, "superseded_by": id(new)?})
        }
        Change::Update { seed, to } => json!({"bank": bank, "id": id(seed)?, "text": text(to)}),
        Change::DeleteBank => json!({"name": bank}),
    })
}

/// Checks what a change's reply says against its payload and returns the
/// seed memory it leaves: a save creates a new memory, and the other calls
/// act on the memory they named.
fn check_reply(change: Change, payload: &Value, reply: &Value) -> Result<Option<(usize, Saved)>> {
    let named = || {
        let memory = stored(reply)?;
        if payload["id"] != memory.id.as_str() {
            bail!(
                "{} acted on {}, not on {}",
                change.function(),
                memory.id,
                payload["id"]
            );
        }
        Ok(memory)
    };
    Ok(match change {
        Change::Save { seed } => Some((seed, created(reply)?)),
        Change::Update { to, .. } => Some((to, named()?)),
        Change::Delete { .. } | Change::Supersede { .. } => {
            named()?;
            None
        }
        Change::RuleSet { .. } | Change::DeleteBank => None,
    })
}

async fn apply(context: &E2eContext, case: &Case, run_id: &str, change: Change) -> Result<()> {
    let (ids, retrieval) = lock_unpoisoned(runs())
        .get(run_id)
        .map(|run| {
            let ids: Vec<Option<String>> = run
                .seeds
                .iter()
                .map(|seed| seed.as_ref().map(|seed| seed.id.clone()))
                .collect();
            (ids, run.retrieval)
        })
        .context("the attempt was never set up")?;
    let payload = change_payload(case, change, run_id, &ids)?;
    let reply = context
        .trigger_value(change.function(), payload.clone())
        .await
        .context(change.function())?;
    let Some((index, memory)) = check_reply(change, &payload, &reply)? else {
        return Ok(());
    };
    keep(run_id, index, memory.clone());
    // Lexical-only stores no vector to wait for.
    if retrieval == Some(Retrieval::Minilm) {
        wait_for_vector(context, &bank(run_id), &memory.id, &memory.text).await?;
    }
    Ok(())
}

/// Applies the changes due once exchange `finished` has its answer, in order,
/// and keeps each outcome for `snapshot`: `before_followup` cannot fail the
/// run, so a failed change becomes an infrastructure error at evaluation.
pub async fn apply_changes(context: &E2eContext, case: &Case, run_id: &str, finished: usize) {
    for &(after, change) in case.changes.iter().filter(|(after, _)| *after == finished) {
        let status = match apply(context, case, run_id, change).await {
            Ok(()) => "ok".to_string(),
            Err(error) => format!("error: {error:#}"),
        };
        if let Some(run) = lock_unpoisoned(runs()).get_mut(run_id) {
            run.changes.push(ChangeOutcome {
                after,
                function: change.function().to_string(),
                status,
            });
        }
    }
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

/// What the attempt recorded so far; empty for an attempt never set up.
pub fn snapshot(run_id: &str) -> Snapshot {
    lock_unpoisoned(runs())
        .get(run_id)
        .map_or_else(Snapshot::default, |run| Snapshot {
            prompts: lock_unpoisoned(&run.prompts).clone(),
            compaction: run.compaction.clone(),
            retrieval: run.retrieval,
            stored: run
                .seeds
                .iter()
                .map(|seed| seed.as_ref().map(|seed| seed.text.clone()))
                .collect(),
            changes: run.changes.clone(),
        })
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
    use crate::scenarios::memory::cases::CHANGING;
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
    fn the_doctor_picks_minilm_lexical_or_refuses_another_embedder() {
        const MINILM: &str =
            "router::embed answered (model sentence-transformers/all-MiniLM-L6-v2@c9745ed1d9f2)";
        // Memory on a Linux stack, whose iii-directory has no MiniLM.
        const LEXICAL: &str =
            "router::embed returned no vector; recall runs lexical-only (BM25 + entities)";
        let doctor = |router: bool, ok: bool, detail: &str| {
            json!({"ok": true, "checks": [
                {"name": "store-roundtrip", "ok": true, "detail": "save→recall→delete ok"},
                {"name": "router-reachable", "ok": router, "detail": "router::models::list reachable"},
                {"name": "state-reachable", "ok": true, "detail": "state::get reachable"},
                {"name": "embeddings-available", "ok": ok, "detail": detail},
            ]})
        };
        assert_eq!(
            embedder(&doctor(true, true, MINILM)),
            Embedder::Ready(Retrieval::Minilm)
        );
        assert_eq!(
            embedder(&doctor(true, false, LEXICAL)),
            Embedder::Ready(Retrieval::Lexical)
        );
        let paid = "router::embed answered (model text-embedding-3-small)";
        assert_eq!(
            embedder(&doctor(true, true, paid)),
            Embedder::Other(paid.to_string())
        );
        // Memory reports a config that turns embeddings off as ok.
        let disabled = "embeddings disabled by config; recall is lexical-only";
        assert_eq!(
            embedder(&doctor(true, true, disabled)),
            Embedder::Ready(Retrieval::Lexical)
        );
        assert_eq!(
            embedder(&doctor(true, false, "router::embed timed out")),
            Embedder::Pending
        );
        // No vector while the router or state is still down says nothing yet.
        assert_eq!(embedder(&doctor(false, false, LEXICAL)), Embedder::Pending);
        let mut state_down = doctor(true, false, LEXICAL);
        state_down["checks"][2]["ok"] = json!(false);
        assert_eq!(embedder(&state_down), Embedder::Pending);
        assert_eq!(embedder(&json!({"checks": []})), Embedder::Pending);
    }

    #[test]
    fn lexical_is_believed_once_it_holds_for_fifteen_seconds() {
        let first = Instant::now();
        assert!(!lexical_settled(first, first));
        assert!(!lexical_settled(first, first + Duration::from_secs(14)));
        assert!(lexical_settled(first, first + Duration::from_secs(15)));
    }

    #[test]
    fn cleanup_ignores_a_bank_that_was_never_created() {
        assert!(bank_already_gone("bank_not_found: e2e-x"));
        assert!(!bank_already_gone("storage: disk full"));
    }

    #[test]
    fn every_change_calls_memory_on_the_attempts_bank() {
        let case = &CHANGING;
        let ids: Vec<Option<String>> = (0..case.memories.len())
            .map(|index| Some(format!("m{index}")))
            .collect();
        let call = |change| change_payload(case, change, "a", &ids).unwrap();
        let text = |index: usize| render(case.memories[index].text, "a");
        assert_eq!(
            call(Change::RuleSet {
                name: "commit-messages",
                content: "Prefix [{after}].",
            }),
            json!({"bank": "e2e-a", "name": "commit-messages",
                   "content": render("Prefix [{after}].", "a")})
        );
        assert_eq!(
            call(Change::Save { seed: 6 }),
            json!({"bank": "e2e-a", "text": text(6), "entities": ["search"]})
        );
        assert_eq!(
            call(Change::Delete { seed: 1 }),
            json!({"bank": "e2e-a", "id": "m1"})
        );
        assert_eq!(
            call(Change::Supersede { old: 2, new: 4 }),
            json!({"bank": "e2e-a", "id": "m2", "superseded_by": "m4"})
        );
        assert_eq!(
            call(Change::Update { seed: 3, to: 5 }),
            json!({"bank": "e2e-a", "id": "m3", "text": text(5)})
        );
        assert_eq!(call(Change::DeleteBank), json!({"name": "e2e-a"}));
        // A seed no save reached has no memory to act on.
        let error = change_payload(case, Change::Delete { seed: 1 }, "a", &[]).unwrap_err();
        assert!(format!("{error:#}").contains("seed 1"), "{error:#}");
    }

    #[test]
    fn a_change_reply_must_show_the_planned_memory() {
        let memory = |id: &str, text: &str| json!({"id": id, "text": text});
        let saved = |id: &str| Saved {
            id: id.into(),
            text: "new text".into(),
        };
        // A save creates; one that reinforced an existing memory is refused.
        let save = Change::Save { seed: 4 };
        let created = json!({"created": true, "memory": memory("m4", "new text")});
        assert_eq!(
            check_reply(save, &json!({}), &created).unwrap(),
            Some((4, saved("m4")))
        );
        let reinforced = json!({"created": false, "memory": memory("m4", "new text")});
        assert!(check_reply(save, &json!({}), &reinforced).is_err());
        // An update keeps the id and leaves the new text with the seed it writes.
        let update = Change::Update { seed: 3, to: 5 };
        let named = json!({"id": "m3"});
        let rewritten = json!({"memory": memory("m3", "new text")});
        assert_eq!(
            check_reply(update, &named, &rewritten).unwrap(),
            Some((5, saved("m3")))
        );
        let other = json!({"memory": memory("m9", "new text")});
        assert!(check_reply(update, &named, &other).is_err());
        // Delete and supersede answer with the memory they retired.
        for change in [
            Change::Delete { seed: 3 },
            Change::Supersede { old: 3, new: 4 },
        ] {
            let retired = json!({"memory": memory("m3", "old text")});
            assert_eq!(check_reply(change, &named, &retired).unwrap(), None);
            assert!(check_reply(change, &named, &other).is_err());
        }
        for change in [
            Change::RuleSet {
                name: "r",
                content: "",
            },
            Change::DeleteBank,
        ] {
            assert_eq!(
                check_reply(change, &json!({}), &json!({"ok": true})).unwrap(),
                None
            );
        }
    }
}
