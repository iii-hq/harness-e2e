//! Plants, observes and removes what one memory attempt needs from the stack:
//! an isolated bank with the case's rules and memories, a teacher session
//! memory learns from, an observer on every prompt Harness is about to send,
//! the case's changes between exchanges, a forced compaction, the bank once
//! memory's extraction read a session, and the cleanup of all of it. Every
//! `memory::*` call names the attempt's bank: left out, memory would act on
//! the user's own `main`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};
use tokio::time::sleep;

use super::cases::{bank, canary, labels, planted, render, Case, Change};
use super::observe::observe;
use super::score::{
    ChangeOutcome, Extraction, PromptRecord, Retrieval, Snapshot, StoredMemory, LEARNED_RULE,
};
use crate::context::E2eContext;
use crate::scenarios::common::lock_unpoisoned;
use crate::wire::{
    FunctionPolicy, MessageInput, SendOptions, SendRequest, SendResponse, SessionInit, TurnStatus,
};

const PRE_GENERATE: &str = "harness::hook::pre-generate";
const EMBEDDER: &str = "all-MiniLM-L6-v2";
const POLL: Duration = Duration::from_secs(2);
const EMBEDDER_WAIT: Duration = Duration::from_secs(300);
const VECTOR_WAIT: Duration = Duration::from_secs(180);
/// How long lexical-only must hold before it is believed.
const LEXICAL_SETTLE: Duration = Duration::from_secs(15);
/// No progress for this long fails a teacher turn.
const TEACHER_WAIT: Duration = Duration::from_secs(300);
/// Memory's extraction call times out at 60 s and the queue retries it.
const EXTRACTION_WAIT: Duration = Duration::from_secs(240);
/// Where memory keeps each session's extraction cursor.
const CURSOR_SCOPE: &str = "memory_cursor";
// shortcut: `setup` has no subject, so the teacher always talks to the
// memory stack's own DeepSeek; pass the subject into setup once this suite
// runs on a stack without it.
const TEACHER_PROVIDER: &str = "deepseek";
const TEACHER_MODEL: &str = "deepseek-flash";

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
    /// The teacher session, from before it exists until cleanup deletes it.
    teacher: Option<String>,
    /// Banks other than the attempt's that memory read on a teacher turn:
    /// extraction wrote what it learned there.
    strays: Vec<String>,
    /// The bank once extraction read the teacher session.
    taught: Option<Extraction>,
}

/// A memory as memory stored it.
#[derive(Debug, Clone, PartialEq)]
struct Saved {
    id: String,
    text: String,
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

/// Fails unless the preview injects `wanted` first.
fn preview_leads(reply: &Value, wanted: &Saved) -> Result<()> {
    let first = reply["memories"][0]["memory"]["id"].as_str();
    if first != Some(wanted.id.as_str()) {
        bail!(
            "case precondition no longer holds: memory::preview injects {} first, not {:?}",
            first.unwrap_or("nothing"),
            wanted.text
        );
    }
    Ok(())
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

/// Registers the attempt with nothing done yet, and returns the sink its
/// observer records prompts into.
fn register(case: &Case, run_id: &str) -> Arc<Mutex<Vec<PromptRecord>>> {
    let prompts = Arc::new(Mutex::new(Vec::new()));
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
            teacher: None,
            strays: Vec::new(),
            taught: None,
        },
    );
    prompts
}

/// Plants the case in its own bank and starts observing the attempt's prompts.
pub async fn setup(context: &E2eContext, case: &Case, run_id: &str) -> Result<()> {
    // Registered first, so `cleanup` undoes whatever a failed setup left.
    let prompts = register(case, run_id);

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
        for &(exchange, seed) in case.preview_first {
            let query = match exchange {
                0 => case.prompt,
                later => case.followups[later - 1],
            };
            let reply = context
                .trigger_value("memory::preview", json!({"bank": bank, "query": query}))
                .await
                .context("memory::preview")?;
            let wanted = lock_unpoisoned(runs())
                .get(run_id)
                .and_then(|run| run.seeds[seed].clone())
                .context("a previewed seed was never planted")?;
            preview_leads(&reply, &wanted)?;
        }
        if !case.teacher.is_empty() {
            teach(context, case, run_id, retrieval).await?;
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

/// The session the case's teacher speaks in.
pub fn teacher(run_id: &str) -> String {
    format!("teach_{run_id}")
}

/// Records the teacher in the attempt before it exists, so `cleanup` deletes
/// it whatever fails after; returns its session id.
fn enroll_teacher(run_id: &str) -> String {
    let session = teacher(run_id);
    if let Some(run) = lock_unpoisoned(runs()).get_mut(run_id) {
        run.teacher = Some(session.clone());
    }
    session
}

/// The send of teacher message `index`: a session of the user's own kind,
/// which memory extracts, pinned to the attempt's bank in its metadata and
/// in the turn's, and with nothing to call, so whatever memory keeps comes
/// from its extraction alone.
fn teacher_send(run_id: &str, index: usize, message: String) -> SendRequest {
    let bank = bank(run_id);
    SendRequest {
        session_id: Some(teacher(run_id)),
        message: MessageInput::Text(message),
        model: Some(TEACHER_MODEL.into()),
        provider: Some(TEACHER_PROVIDER.into()),
        idempotency_key: Some(format!("e2e:{run_id}:teacher:send:{index}")),
        session: (index == 0).then(|| SessionInit {
            kind: Some("user".into()),
            title: Some("Harness E2E memory teacher".into()),
            metadata: Some(
                json!({"memory_bank": bank, "e2e_run_id": run_id, "e2e_role": "teacher"}),
            ),
        }),
        options: Some(SendOptions {
            max_turns: Some(1),
            functions: Some(FunctionPolicy {
                allow: Vec::new(),
                deny: vec!["memory::*".into()],
                ..FunctionPolicy::default()
            }),
            metadata: Some(json!({"memory_bank": bank})),
            ..SendOptions::default()
        }),
    }
}

/// The bank memory read on each of the teacher's turns in `transcript`;
/// every one other than the attempt's is kept for `cleanup` to purge.
fn record_strays(run_id: &str, transcript: &Value) -> Vec<Option<String>> {
    let bank = bank(run_id);
    let read: Vec<Option<String>> = observe(transcript)
        .exchanges
        .into_iter()
        .map(|exchange| exchange.memory_bank)
        .collect();
    if let Some(run) = lock_unpoisoned(runs()).get_mut(run_id) {
        for stray in read.iter().flatten().filter(|read| **read != bank) {
            if !run.strays.contains(stray) {
                run.strays.push(stray.clone());
            }
        }
    }
    read
}

/// Sends the case's teacher messages, waits until memory's extraction has
/// read them, and keeps the bank as it then stands.
async fn teach(
    context: &E2eContext,
    case: &Case,
    run_id: &str,
    retrieval: Retrieval,
) -> Result<()> {
    let session = enroll_teacher(run_id);
    let bank = bank(run_id);
    let sent: Result<()> = async {
        for (index, template) in case.teacher.iter().enumerate() {
            let response: SendResponse = context
                .trigger(
                    "harness::send",
                    teacher_send(run_id, index, render(template, run_id)),
                )
                .await
                .context("harness::send to the teacher")?;
            if !response.accepted
                || response.session_id != session
                || response.merged == Some(true)
                || response.queued == Some(true)
            {
                bail!("harness::send to the teacher returned an unexpected response: {response:?}");
            }
            context
                .wait_for_turn(
                    case.id,
                    &session,
                    &response.turn_id,
                    TurnStatus::Completed,
                    TEACHER_WAIT,
                    false,
                    None,
                )
                .await
                .with_context(|| format!("teacher turn {index}"))?;
        }
        Ok(())
    }
    .await;
    // Read even when a turn failed: the turns before it may have been read
    // on another bank, and cleanup purges whatever is recorded here.
    let transcript = context
        .transcript(&session)
        .await
        .context("the teacher's transcript");
    let read = match &transcript {
        Ok(transcript) => record_strays(run_id, transcript),
        Err(_) => Vec::new(),
    };
    sent?;
    let transcript = transcript?;
    // Waited for even when memory read another bank: cleanup then retires
    // what extraction wrote there once it is all in.
    let taught = extraction(context, &session, &bank, &transcript).await?;
    if read.len() != case.teacher.len() || read.iter().any(|read| read.as_deref() != Some(&bank)) {
        bail!("memory did not read the attempt's bank on every teacher turn: {read:?}");
    }
    if let Some(other) = case
        .teacher_reply
        .and_then(|reply| unexpected_reply(&transcript, reply))
    {
        bail!("case precondition no longer holds: the teacher replied {other:?}");
    }
    // Lexical-only stores no vector to wait for.
    if retrieval == Retrieval::Minilm {
        for memory in taught.learned() {
            wait_for_vector(context, &bank, &memory.id, &memory.text).await?;
        }
    }
    if let Some(run) = lock_unpoisoned(runs()).get_mut(run_id) {
        run.taught = Some(taught);
    }
    Ok(())
}

/// The first teacher reply in `transcript` other than `reply`, ignoring case
/// and the punctuation around the word. Only text is a reply: thinking never
/// reaches extraction.
fn unexpected_reply(transcript: &Value, reply: &str) -> Option<String> {
    transcript["messages"]
        .as_array()?
        .iter()
        .filter(|entry| entry.pointer("/message/role").and_then(Value::as_str) == Some("assistant"))
        .map(|entry| {
            entry["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<String>()
        })
        .filter(|text| !text.trim().is_empty())
        .find(|text| {
            !text
                .trim_matches(|c: char| !c.is_alphanumeric())
                .eq_ignore_ascii_case(reply)
        })
}

/// The entry a `state::get` of the cursor names: memory stores
/// `{last_entry_id}`, and older engines return it under `value`.
pub fn cursor_entry(reply: &Value) -> Option<&str> {
    reply
        .pointer("/last_entry_id")
        .or_else(|| reply.pointer("/value/last_entry_id"))
        .and_then(Value::as_str)
}

/// The entry memory's extraction cursor stops at once it read the session:
/// its newest user or assistant message. Custom entries (notices, memory's
/// own blocks) and function results never are.
pub fn cursor_target(transcript: &Value) -> Option<&str> {
    transcript["messages"]
        .as_array()?
        .iter()
        .rev()
        .find(|entry| {
            matches!(
                entry.pointer("/message/role").and_then(Value::as_str),
                Some("user" | "assistant")
            )
        })?["entry_id"]
        .as_str()
}

/// Waits until memory's extraction has read `session` through `entry`, and
/// says how long that took. The cursor moves once a pass is done, whatever
/// it kept, and never when a pass failed, so it is the completion signal.
async fn wait_cursor(context: &E2eContext, session: &str, entry: &str) -> Result<Duration> {
    let started = Instant::now();
    loop {
        let reply = context
            .trigger_value("state::get", json!({"scope": CURSOR_SCOPE, "key": session}))
            .await;
        if reply.as_ref().ok().and_then(cursor_entry) == Some(entry) {
            return Ok(started.elapsed());
        }
        if started.elapsed() >= EXTRACTION_WAIT {
            // A diagnostic only: memory's doctor reports the last pass of any session.
            let passes = context
                .trigger_value("memory::doctor", json!({}))
                .await
                .ok()
                .and_then(|doctor| {
                    doctor["checks"]
                        .as_array()?
                        .iter()
                        .find(|check| check["name"] == "extraction-passes")
                        .map(|check| check["detail"].to_string())
                });
            bail!(
                "memory's extraction never read {session} through {entry} in {}s (cursor: {}; doctor extraction-passes: {})",
                EXTRACTION_WAIT.as_secs(),
                reply.map_or_else(|error| format!("{error:#}"), |reply| reply.to_string()),
                passes.unwrap_or_else(|| "unknown".into())
            );
        }
        sleep(POLL).await;
    }
}

/// The memories a `memory::list` reply holds.
pub fn memories_of(list: &Value) -> Vec<StoredMemory> {
    let text = |memory: &Value, field: &str| memory[field].as_str().unwrap_or_default().to_string();
    list["memories"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|memory| StoredMemory {
            id: text(memory, "id"),
            text: text(memory, "text"),
            session_id: memory["source"]["session_id"].as_str().map(str::to_string),
            live: memory["invalid_at"].is_null(),
        })
        .collect()
}

/// Rule name -> content, from a `memory::rule::list` reply.
pub fn rules_of(reply: &Value) -> BTreeMap<String, String> {
    reply["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|rule| {
            Some((
                rule["name"].as_str()?.to_string(),
                rule["content"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Waits until memory's extraction has read `session`'s `transcript`, then
/// reads `bank`, superseded memories included.
async fn extraction(
    context: &E2eContext,
    session: &str,
    bank: &str,
    transcript: &Value,
) -> Result<Extraction> {
    let entry = cursor_target(transcript)
        .with_context(|| format!("{session} holds no message for memory to read"))?;
    let waited = wait_cursor(context, session, entry).await?;
    let list = context
        .trigger_value(
            "memory::list",
            json!({"bank": bank, "include_superseded": true, "limit": 500}),
        )
        .await
        .context("memory::list")?;
    let rules = context
        .trigger_value("memory::rule::list", json!({"bank": bank}))
        .await
        .context("memory::rule::list")?;
    Ok(Extraction {
        session_id: session.to_string(),
        waited_ms: u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
        memories: memories_of(&list),
        rules: rules_of(&rules),
    })
}

/// The bank once memory's extraction has read the measured session, whose
/// transcript `capture` holds.
pub async fn bank_after(
    context: &E2eContext,
    run_id: &str,
    transcript: &Value,
) -> Result<Extraction> {
    extraction(context, &format!("e2e_{run_id}"), &bank(run_id), transcript).await
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
            teacher: run.taught.clone(),
            bank_after: None,
        })
}

/// One thing `cleanup` undoes.
#[derive(Debug, PartialEq)]
enum Undo {
    /// The teacher session, which the Console lists with the user's own.
    DeleteTeacher(String),
    /// What the teacher's extraction wrote into a bank memory read instead
    /// of the attempt's.
    Purge {
        bank: String,
        session: String,
    },
    DeleteBank(String),
}

/// What `cleanup` undoes for an attempt, in order: the teacher before the
/// bank, whatever setup got to.
fn undo(run: Option<&Run>, run_id: &str) -> Vec<Undo> {
    let teacher = run.and_then(|run| run.teacher.clone());
    let strays = run.map_or(&[][..], |run| run.strays.as_slice());
    let purges = teacher.iter().flat_map(|session| {
        strays.iter().map(|bank| Undo::Purge {
            bank: bank.clone(),
            session: session.clone(),
        })
    });
    teacher
        .clone()
        .map(Undo::DeleteTeacher)
        .into_iter()
        .chain(purges)
        .chain([Undo::DeleteBank(bank(run_id))])
        .collect()
}

/// `content` without the lines holding any of `canaries`, when it had one.
fn without_lines(content: &str, canaries: &[String]) -> Option<String> {
    let kept: Vec<&str> = content
        .lines()
        .filter(|line| !canaries.iter().any(|canary| line.contains(canary.as_str())))
        .collect();
    (kept.len() != content.lines().count()).then(|| kept.join("\n"))
}

/// Retires what `session`'s extraction wrote into `bank`: its memories, and
/// the learned lines holding the run's canaries. Read first-page only: memory
/// lists the newest first, and the writes are minutes old.
async fn purge(context: &E2eContext, bank: &str, session: &str, canaries: &[String]) -> Result<()> {
    let list = context
        .trigger_value("memory::list", json!({"bank": bank, "limit": 500}))
        .await
        .context("memory::list")?;
    let written = memories_of(&list)
        .into_iter()
        .filter(|memory| memory.session_id.as_deref() == Some(session));
    for memory in written {
        context
            .trigger_value("memory::delete", json!({"bank": bank, "id": memory.id}))
            .await
            .context("memory::delete")?;
    }
    let rules = context
        .trigger_value("memory::rule::list", json!({"bank": bank}))
        .await
        .context("memory::rule::list")?;
    let learned = rules_of(&rules).remove(LEARNED_RULE);
    if let Some(content) = learned.and_then(|content| without_lines(&content, canaries)) {
        context
            .trigger_value(
                "memory::rule::set",
                json!({"bank": bank, "name": LEARNED_RULE, "content": content}),
            )
            .await
            .context("memory::rule::set")?;
    }
    Ok(())
}

/// Unbinds the observer, deletes the teacher, retires what its extraction
/// wrote into another bank, and trashes the attempt's bank. Every step runs;
/// the first error is returned.
pub async fn cleanup(context: &E2eContext, case: &Case, run_id: &str) -> Result<()> {
    let run = lock_unpoisoned(runs()).remove(run_id);
    let steps = undo(run.as_ref(), run_id);
    if let Some(run) = run {
        if let Some(trigger) = run.trigger {
            trigger.unregister();
        }
        if let Some(function) = run.function {
            function.unregister();
        }
    }
    let canaries: Vec<String> = labels(case)
        .into_iter()
        .map(|label| canary(run_id, label))
        .collect();
    let mut outcome = Ok(());
    for step in steps {
        let done = match step {
            Undo::DeleteTeacher(session) => {
                // A turn a failed setup left running; stopping an idle one is a no-op.
                let _ = context.stop_session(&session, None).await;
                // Memory extracts a failed or stopped turn too, and `teach` skips
                // its wait when a turn failed: a pass landing after the purge
                // below would leave its writes in a stray bank. Best effort.
                if let Ok(transcript) = context.transcript(&session).await {
                    if let Some(entry) = cursor_target(&transcript) {
                        let _ = wait_cursor(context, &session, entry).await;
                    }
                }
                context
                    .trigger_value("session::delete", json!({"session_id": session}))
                    .await
                    .context("session::delete")
                    .map(drop)
            }
            // Only what this attempt's teacher wrote: memories whose source
            // session is the teacher, and learned lines holding this run's canaries.
            Undo::Purge { bank, session } => purge(context, &bank, &session, &canaries).await,
            Undo::DeleteBank(name) => match context
                .trigger_value("memory::bank::delete", json!({"name": name}))
                .await
            {
                Err(error) if !bank_already_gone(&format!("{error:#}")) => {
                    Err(error).context("memory::bank::delete")
                }
                _ => Ok(()),
            },
        };
        if outcome.is_ok() {
            outcome = done;
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::memory::cases::{CASES, CHANGING};
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
    fn a_preview_must_inject_the_planned_seed_first() {
        let wanted = Saved {
            id: "fp-b".into(),
            text: "To migrate the ledger database, run migrate-x.".into(),
        };
        let preview = |ids: &[&str]| {
            let memories: Vec<Value> = ids
                .iter()
                .map(|id| json!({"memory": {"id": id, "text": "t"}, "score": 1.0}))
                .collect();
            json!({"bank": "e2e-a", "memories": memories, "retrieval": "bm25-entity"})
        };
        preview_leads(&preview(&["fp-b"]), &wanted).unwrap();
        preview_leads(&preview(&["fp-b", "fp-a"]), &wanted).unwrap();
        for reply in [preview(&["fp-a"]), preview(&["fp-a", "fp-b"]), preview(&[])] {
            let error = preview_leads(&reply, &wanted).unwrap_err().to_string();
            assert!(
                error.starts_with("case precondition no longer holds")
                    && error.contains(&wanted.text),
                "{error}"
            );
        }
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

    #[test]
    fn the_cursor_wait_matches_the_stored_shape() {
        // Memory stores `{last_entry_id}`; older engines return it under `value`.
        let entry = "e_t_b_0_assistant";
        assert_eq!(cursor_entry(&json!({"last_entry_id": entry})), Some(entry));
        assert_eq!(
            cursor_entry(&json!({"value": {"last_entry_id": entry}})),
            Some(entry)
        );
        // The id alone, or nothing stored yet, is no cursor.
        for reply in [
            json!(entry),
            json!(null),
            json!({}),
            json!({"value": entry}),
        ] {
            assert_eq!(cursor_entry(&reply), None, "{reply}");
        }
        // The cursor stops at the newest user or assistant message, never at
        // the notices, memory's blocks or function results after it.
        let transcript = json!({"messages": [
            {"entry_id": "e_t_a_user", "message": {"role": "user", "content": []}},
            {"entry_id": "e_t_a_0_assistant", "message": {"role": "assistant", "content": []}},
            {"entry_id": "e_t_b_user", "message": {"role": "user", "content": []}},
            {"entry_id": "e_t_b_0_notice_0", "custom": {"custom_type": "model_notice",
             "data": {"kind": "hook", "text": "<memory bank=\"e2e-a\">\n</memory>"}}},
            {"entry_id": entry, "message": {"role": "assistant", "content": []},
             "origin": {"turn_id": "t_b"}},
            {"entry_id": "e_t_b_0_result", "message": {"role": "function_result", "content": []}},
            {"entry_id": "e_t_b_notice", "message": {"role": "custom", "custom_type": "notice", "content": []}},
        ]});
        assert_eq!(cursor_target(&transcript), Some(entry));
        assert_eq!(cursor_target(&json!({"messages": []})), None);
    }

    #[test]
    fn the_teacher_session_is_deleted_even_when_setup_fails_after_it() {
        let case = CASES.iter().find(|case| !case.teacher.is_empty()).unwrap();
        let run_id = "teacher-cleanup";
        // Setup's first step, then the teacher's, before any send.
        register(case, run_id);
        let session = enroll_teacher(run_id);
        assert_eq!(session, teacher(run_id));
        // The observer records `e2e_<attempt>` only, never the teacher.
        assert_ne!(session, format!("e2e_{run_id}"));
        // The send, the turn or the extraction wait fails: nothing else ran.
        let mut run = lock_unpoisoned(runs()).remove(run_id).unwrap();
        assert_eq!(
            undo(Some(&run), run_id),
            [
                Undo::DeleteTeacher(session.clone()),
                Undo::DeleteBank(bank(run_id))
            ]
        );
        // Memory read another bank on a teacher turn: what extraction wrote
        // there goes too, before the attempt's bank.
        run.strays = vec!["main".into()];
        assert_eq!(
            undo(Some(&run), run_id),
            [
                Undo::DeleteTeacher(session.clone()),
                Undo::Purge {
                    bank: "main".into(),
                    session
                },
                Undo::DeleteBank(bank(run_id))
            ]
        );
        // No teacher, or no setup at all: the bank alone.
        run.teacher = None;
        run.strays.clear();
        assert_eq!(undo(Some(&run), run_id), [Undo::DeleteBank(bank(run_id))]);
        assert_eq!(undo(None, run_id), [Undo::DeleteBank(bank(run_id))]);
    }

    #[test]
    fn a_teach_failing_on_a_later_turn_leaves_the_earlier_strays_for_cleanup() {
        let case = CASES.iter().find(|case| case.teacher.len() > 1).unwrap();
        let run_id = "teacher-strays";
        register(case, run_id);
        let session = enroll_teacher(run_id);
        // Turn 0 was read on `main`; turn 1 then failed, so its transcript
        // holds turn 0 alone.
        let turn = |id: &str, bank: &str| {
            json!({"entry_id": format!("e_{id}_0_assistant"),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
                   "origin": {"turn_id": id, "memory_bank": bank}})
        };
        let read = record_strays(run_id, &json!({"messages": [turn("t_0", "main")]}));
        assert_eq!(read, [Some("main".to_string())]);
        // A later read naming the same stray bank and the attempt's adds nothing.
        record_strays(
            run_id,
            &json!({"messages": [turn("t_0", "main"), turn("t_1", &bank(run_id))]}),
        );
        let run = lock_unpoisoned(runs()).remove(run_id).unwrap();
        assert_eq!(
            undo(Some(&run), run_id),
            [
                Undo::DeleteTeacher(session.clone()),
                Undo::Purge {
                    bank: "main".into(),
                    session
                },
                Undo::DeleteBank(bank(run_id))
            ]
        );
    }

    #[test]
    fn the_teacher_is_a_user_session_pinned_to_the_bank_with_nothing_to_call() {
        let first = serde_json::to_value(teacher_send("a", 0, "Remember this.".into())).unwrap();
        assert_eq!(first["session_id"], "teach_a");
        assert_eq!(first["message"], "Remember this.");
        assert_eq!(first["session"]["kind"], "user");
        assert_eq!(first["session"]["metadata"]["memory_bank"], "e2e-a");
        assert_eq!(
            first["options"]["metadata"],
            json!({"memory_bank": "e2e-a"})
        );
        assert_eq!(first["options"]["functions"]["allow"], json!([]));
        let second = serde_json::to_value(teacher_send("a", 1, "And this.".into())).unwrap();
        assert!(second.get("session").is_none());
        assert_ne!(first["idempotency_key"], second["idempotency_key"]);
    }

    #[test]
    fn a_teacher_reply_other_than_the_fixed_one_is_reported() {
        let reply = |text: &str| {
            json!({"entry_id": "e_t_0_assistant", "origin": {"turn_id": "t"},
                   "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
        };
        let user = json!({"entry_id": "e_t_user",
                          "message": {"role": "user", "content": [{"type": "text", "text": "db-0a1b2c3d"}]}});
        let transcript = |replies: &[&str]| {
            let mut messages = vec![user.clone()];
            messages.extend(replies.iter().map(|text| reply(text)));
            json!({ "messages": messages })
        };
        // Case and the punctuation around the word do not count.
        let fixed: [&[&str]; 4] = [&["noted"], &["Noted.", "**noted**"], &["noted", "   "], &[]];
        for fixed in fixed {
            assert_eq!(
                unexpected_reply(&transcript(fixed), "noted"),
                None,
                "{fixed:?}"
            );
        }
        // A reply that restates what the user said, or refuses to keep it.
        for other in [
            "Noted: the staging host is db-0a1b2c3d.",
            "I won't remember that.",
        ] {
            assert_eq!(
                unexpected_reply(&transcript(&["noted", other]), "noted").as_deref(),
                Some(other)
            );
        }
        // Thinking is not a reply.
        let mut thought = transcript(&["noted"]);
        thought["messages"][1]["message"]["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type": "thinking", "thinking": "the host is db-0a1b2c3d"}));
        assert_eq!(unexpected_reply(&thought, "noted"), None);
    }

    #[test]
    fn a_bank_is_read_with_its_retired_memories_and_its_rules() {
        let list = json!({"total": 2, "memories": [
            {"id": "fp1", "text": "Learned.", "source": {"session_id": "teach_a", "entry_id": "e"},
             "created_at": 1, "updated_at": 1},
            {"id": "fp2", "text": "Saved.", "invalid_at": 5, "created_at": 1, "updated_at": 5},
        ]});
        assert_eq!(
            memories_of(&list),
            [
                StoredMemory {
                    id: "fp1".into(),
                    text: "Learned.".into(),
                    session_id: Some("teach_a".into()),
                    live: true,
                },
                StoredMemory {
                    id: "fp2".into(),
                    text: "Saved.".into(),
                    session_id: None,
                    live: false,
                },
            ]
        );
        let rules =
            json!({"bank": "e2e-a", "rules": [{"name": "learned", "content": "# Learned\n- x\n"}]});
        assert_eq!(
            rules_of(&rules),
            BTreeMap::from([("learned".to_string(), "# Learned\n- x\n".to_string())])
        );
        // A purge drops only the learned lines holding the run's canaries.
        let canaries = ["0a1b2c3d".to_string()];
        assert_eq!(
            without_lines("# Learned\n- keep\n- drop 0a1b2c3d\n", &canaries),
            Some("# Learned\n- keep".into())
        );
        assert_eq!(without_lines("# Learned\n- keep\n", &canaries), None);
    }
}
