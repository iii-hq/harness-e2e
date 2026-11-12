//! Finite final-SDK witnesses, not provider receipts or a global event watermark.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iii_sdk::{IIIClient, ObserverHandle, ObserverState, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::context::E2eContext;

const REQUEST: &str = "harness::on-generation-request";
const RESPONSE: &str = "harness::on-generation-response";
const UPDATE: &str = "session::update-message";
const NOTICE: &str = "session::on-message-added";
const HOOK: &str = "memory::hook::pre-generate";
const APPEND: &str = "session::append";
const RECEIPT: &str = "harness::on-synthetic-memory-receipt";
const SOURCES: [&str; 7] = [REQUEST, RESPONSE, UPDATE, NOTICE, HOOK, APPEND, RECEIPT];
const LIMIT: usize = 8192;
const GENERATION_LIMIT: usize = 256;
const HASH_LIMIT: usize = 32768;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub role: String,
    pub location: Vec<usize>,
    pub line: usize,
    pub sha256: String,
}

/// Wire-compatible with the audited final-SDK producer; legacy witnesses never credit delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u32,
    pub producer_instance: String,
    pub generation_id: String,
    pub assistant_entry_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub step: u64,
    pub dispatch_index: u64,
    pub sequence: u64,
    pub phase: String,
    pub complete: bool,
    pub message_count: usize,
    pub text_bytes: usize,
    pub lines: Vec<Line>,
    pub sources: Vec<SourceSlot>,
    pub origins_complete: bool,
    #[serde(deserialize_with = "required_summary")]
    pub summary: Option<SummaryWitness>,
}

fn required_summary<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<SummaryWitness>, D::Error> {
    Option::<SummaryWitness>::deserialize(d)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSlot {
    pub message_index: usize,
    pub entry_id: String,
    pub message_sha256: String,
    pub observer_id: String,
    pub append_ordinal: usize,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummarySpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub summary_sha256: String,
    pub system_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWitness {
    pub assembly_id: String,
    pub span: SummarySpan,
    pub inputs: Vec<SourceSlot>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryReceipt {
    version: u32,
    receipt_id: String,
    producer_instance: String,
    session_id: String,
    turn_id: String,
    step: u64,
    observer_id: String,
    output_sha256: String,
    appends: Vec<AppendReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppendReceipt {
    append_ordinal: usize,
    message_sha256: String,
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_slot(slot: &SourceSlot) -> bool {
    !slot.entry_id.is_empty()
        && slot.entry_id.len() <= 256
        && !slot.observer_id.is_empty()
        && slot.observer_id.len() <= 256
        && slot.message_index < 2048
        && slot.append_ordinal < 2048
        && digest(&slot.message_sha256)
        && matches!(slot.source.as_str(), "fresh_memory" | "replay_memory")
}

fn valid_origins(event: &Event) -> bool {
    event.version == 2
        && event.origins_complete
        && event.sources.len() <= 2048
        && event
            .sources
            .iter()
            .all(|slot| valid_slot(slot) && slot.message_index < event.message_count)
        && event
            .sources
            .iter()
            .map(|slot| slot.message_index)
            .collect::<BTreeSet<_>>()
            .len()
            == event.sources.len()
        && event.summary.as_ref().is_none_or(|summary| {
            !summary.assembly_id.is_empty()
                && summary.assembly_id.len() <= 256
                && summary.span.byte_start < summary.span.byte_end
                && summary.span.byte_end <= event.text_bytes
                && digest(&summary.span.summary_sha256)
                && digest(&summary.span.system_sha256)
                && !summary.inputs.is_empty()
                && summary.inputs.len() <= 2048
                && summary.inputs.iter().all(valid_slot)
                && !summary.lines.is_empty()
                && summary.lines.len() <= 2048
                && summary
                    .lines
                    .windows(2)
                    .all(|pair| pair[0].line < pair[1].line)
                && summary.lines.iter().all(|line| {
                    line.role == "system"
                        && line.location.is_empty()
                        && digest(&line.sha256)
                        && (event.phase != "request" || event.lines.contains(line))
                })
        })
}

fn message_hash(message: &Value) -> Option<String> {
    Some(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(message).ok()?)
    ))
}

// Keep the exact identity; trimming is only an emptiness check.
fn update_generation(input: &Value) -> Option<&str> {
    input
        .pointer("/origin/generation_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
}

fn uuid4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version_num() == 4)
}

fn valid(event: &Event) -> bool {
    matches!(event.version, 1 | 2)
        && (event.version == 1 || valid_origins(event))
        && uuid4(&event.producer_instance)
        && uuid4(&event.generation_id)
        && !event.session_id.is_empty()
        && !event.turn_id.is_empty()
        && event.session_id.len() <= 256
        && event.turn_id.len() <= 256
        && event.assistant_entry_id.len() <= 512
        && event.assistant_entry_id == format!("e_{}_{}_assistant", event.turn_id, event.step)
        && event.sequence > 0
        && event.complete
        && event.lines.len() <= 2048
        && event.message_count <= 2048
        && event.text_bytes <= 1_048_576
        && event.lines.iter().all(|line| {
            matches!(
                line.role.as_str(),
                "system" | "user" | "assistant" | "function_result" | "custom"
            ) && line.location.len() <= 16
                && ((line.role == "system" && line.location.is_empty())
                    || line
                        .location
                        .first()
                        .is_some_and(|i| *i < event.message_count))
                && line.sha256.len() == 64
                && line
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn paired(request: &Event, terminal: &Event) -> bool {
    let mut expected = request.clone();
    expected.phase = "success".into();
    expected.sequence = terminal.sequence;
    expected.lines.clear();
    // Native terminals deliberately omit request-time source certificates.
    if terminal.version == 2 && terminal.sources.is_empty() {
        expected.sources.clear();
    }
    if terminal.version == 2 && terminal.summary.is_none() {
        expected.summary = None;
    }
    valid(request)
        && valid(terminal)
        && request.phase == "request"
        && request.sequence < terminal.sequence
        && expected == *terminal
}

/// Exactly Rust lines/Unicode trim/ONE bullet removal; no case or Unicode folding.
pub fn line_hash(line: &str) -> String {
    let trimmed = line.trim();
    let plain = ["- ", "* ", "+ "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))
        .unwrap_or(trimmed)
        .trim();
    format!("{:x}", Sha256::digest(plain.as_bytes()))
}

#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub ready: bool,
    session: String,
    bank: String,
    pub invalid: bool,
    events: BTreeMap<(String, u64), Event>,
    foreign_sequences: BTreeSet<(String, u64)>,
    foreign_generations: BTreeSet<(String, String)>,
    // Tombstones never roll over within this bound capture/owner epoch.
    // Once full, seal admission, not already retained independent proofs.
    rejections_sealed: bool,
    rejected_generations: BTreeSet<(String, String)>,
    rejected_entries: BTreeSet<String>,
    rejected_updates: BTreeSet<(String, String)>,
    rejected_notices: BTreeSet<String>,
    updates: BTreeMap<String, Vec<Value>>,
    notices: Vec<Value>,
    hooks: Vec<Value>,
    appends: Vec<Value>,
    receipts: BTreeMap<String, MemoryReceipt>,
    rejected_receipts: BTreeSet<String>,
}

impl Captured {
    pub(crate) fn authenticated_bank(&self) -> Option<&str> {
        // Only the private capture adapter sets this bank, never wire actors.
        (self.ready && self.bank == "synthetic-atlas").then_some(self.bank.as_str())
    }

    pub fn run_id(&self) -> &str {
        self.bank.strip_prefix("e2e-").unwrap_or_default()
    }

    pub fn available(&self) -> bool {
        self.ready
            && !self.invalid
            && self.events.values().any(|request| {
                request.session_id == self.session
                    && !self.rejected_generations.contains(&(
                        request.producer_instance.clone(),
                        request.generation_id.clone(),
                    ))
                    && !self.rejected_entries.contains(&request.assistant_entry_id)
                    && !self.rejected_updates.contains(&(
                        request.assistant_entry_id.clone(),
                        request.generation_id.clone(),
                    ))
                    && request.phase == "request"
                    && self
                        .events
                        .values()
                        .any(|terminal| paired(request, terminal))
                    && self
                        .updates
                        .get(&request.assistant_entry_id)
                        .is_some_and(|updates| {
                            updates.iter().any(|update| {
                                update_generation(update) == Some(request.generation_id.as_str())
                            })
                        })
            })
    }
    fn reject(&mut self, payload: &Value) {
        if self.rejected_generations.len()
            + self.rejected_entries.len()
            + self.rejected_notices.len()
            + self.rejected_updates.len()
            >= LIMIT
        {
            self.rejections_sealed = true;
            // No eviction can turn an old rejection into a clean replay: this
            // epoch admits nothing further. Still revoke any retained proof
            // implicated by this or a later authenticated own conflict.
            if let (Some(instance), Some(generation)) = (
                payload["producer_instance"].as_str(),
                payload["generation_id"].as_str(),
            ) {
                self.events.retain(|_, event| {
                    event.producer_instance != instance
                        || (event.generation_id != generation
                            && Some(event.sequence) != payload["sequence"].as_u64())
                });
            } else if let Some(entry) = payload.pointer("/input/entry_id").and_then(Value::as_str) {
                let generation = payload.get("input").and_then(update_generation);
                if generation.is_none()
                    || self.updates.get(entry).is_some_and(|updates| {
                        updates
                            .last()
                            .is_some_and(|update| update_generation(update) == generation)
                    })
                {
                    self.updates.remove(entry);
                }
            } else if let Some(entry) = payload["entry_id"].as_str() {
                self.notices.retain(|notice| notice["entry_id"] != entry);
            } else {
                self.invalid = true;
            }
        } else if let (Some(instance), Some(generation)) = (
            payload["producer_instance"].as_str(),
            payload["generation_id"].as_str(),
        ) {
            self.rejected_generations
                .insert((instance.into(), generation.into()));
        } else if let Some(entry) = payload.pointer("/input/entry_id").and_then(Value::as_str) {
            if let Some(generation) = payload.get("input").and_then(update_generation) {
                self.rejected_updates
                    .insert((entry.into(), generation.into()));
            } else {
                self.rejected_entries.insert(entry.into());
            }
        } else if let Some(entry) = payload["entry_id"].as_str() {
            self.rejected_notices.insert(entry.into());
        } else {
            // Authenticated own data without a usable identity cannot prove delivery.
            self.invalid = true;
        }
    }

    // Booleans come only from the bound SDK invocation/owner checks below,
    // never from declared payload fields. Keep quarantine before attribution.
    fn record_callback(
        &mut self,
        source: &str,
        observed: Value,
        authenticated_source: bool,
        authentic: bool,
    ) {
        if !authenticated_source {
            return;
        }
        // Streamed partials carry no generation and prove nothing; only
        // generation-stamped updates are evidence, so they are never tombstones.
        if source == UPDATE && observed.get("input").and_then(update_generation).is_none() {
            return;
        }
        if authentic {
            self.record(source, observed);
        } else {
            let subject = if matches!(source, UPDATE | HOOK | APPEND) {
                observed.pointer("/input/session_id")
            } else {
                observed.get("session_id")
            }
            .and_then(Value::as_str);
            if subject == Some(self.session.as_str()) {
                self.reject(&observed);
            }
        }
    }

    fn record(&mut self, source: &str, payload: Value) {
        let subject = if matches!(source, UPDATE | HOOK | APPEND) {
            payload.pointer("/input/session_id")
        } else {
            payload.get("session_id")
        }
        .and_then(Value::as_str);
        // This method receives only authenticated callbacks. Unknown subjects
        // cannot be attributed to this capture; foreign malformed data is not
        // evidence against its own session, regardless of arrival order.
        if subject != Some(self.session.as_str()) {
            if matches!(source, REQUEST | RESPONSE) {
                if let (Some(instance), Some(sequence), Some(generation)) = (
                    payload["producer_instance"].as_str(),
                    payload["sequence"].as_u64(),
                    payload["generation_id"].as_str(),
                ) {
                    let key = (instance.to_string(), sequence);
                    let identity = (instance.to_string(), generation.to_string());
                    if self.events.values().any(|own| {
                        own.producer_instance == instance
                            && (own.sequence == sequence || own.generation_id == generation)
                    }) {
                        // Only the colliding generation from this producer is ambiguous.
                        let affected: Vec<_> = self
                            .events
                            .values()
                            .filter(|own| {
                                own.producer_instance == instance
                                    && (own.sequence == sequence || own.generation_id == generation)
                            })
                            .map(|own| (own.producer_instance.clone(), own.generation_id.clone()))
                            .collect();
                        for (instance, generation) in affected {
                            self.reject(&json!({
                                "producer_instance": instance,
                                "generation_id": generation,
                            }));
                        }
                    }
                    let added = usize::from(!self.foreign_sequences.contains(&key))
                        + usize::from(!self.foreign_generations.contains(&identity));
                    if self.foreign_sequences.len() + self.foreign_generations.len() + added
                        <= LIMIT
                    {
                        self.foreign_sequences.insert(key);
                        self.foreign_generations.insert(identity);
                    }
                    // Foreign identities beyond this finite cache are not
                    // retained, but every collision with retained own events
                    // above is still rejected. No global rollover poison.
                }
            }
            return;
        }
        // Only authenticated own subjects reach this gate. Foreign identities
        // and unauthenticated claimed sessions cannot seal this capture epoch.
        // Recovery requires a new bind with current owners and active handles;
        // neither payload UUIDs nor sequence numbers authorize a reset.
        if self.rejections_sealed {
            let retained = match source {
                REQUEST | RESPONSE => serde_json::from_value::<Event>(payload.clone())
                    .ok()
                    .is_some_and(|event| {
                        self.events
                            .get(&(event.producer_instance.clone(), event.sequence))
                            == Some(&event)
                            && (source == REQUEST) == (event.phase == "request")
                    }),
                UPDATE => {
                    payload.pointer("/output/updated") == Some(&json!(true))
                        && payload
                            .pointer("/input/entry_id")
                            .and_then(Value::as_str)
                            .and_then(|entry| self.updates.get(entry))
                            .and_then(|updates| updates.last())
                            .is_some_and(|update| Some(update) == payload.get("input"))
                }
                NOTICE => self.notices.contains(&payload),
                HOOK => self.hooks.contains(&payload),
                RECEIPT => false,
                APPEND => payload
                    .get("input")
                    .is_some_and(|input| self.appends.contains(input)),
                _ => false,
            };
            if !retained {
                self.reject(&payload);
            }
            return;
        }
        if self.events.len()
            + self.notices.len()
            + self.hooks.len()
            + self.appends.len()
            + self.receipts.len()
            + self.rejected_receipts.len()
            + self.updates.values().map(Vec::len).sum::<usize>()
            >= LIMIT
            || self
                .events
                .values()
                .map(|event| event.lines.len())
                .sum::<usize>()
                + payload
                    .get("lines")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
                > HASH_LIMIT
            || (matches!(source, REQUEST | RESPONSE)
                && !self.events.values().any(|event| {
                    payload["generation_id"] == event.generation_id
                        && payload["producer_instance"] == event.producer_instance
                })
                && self
                    .events
                    .values()
                    .map(|event| (&event.producer_instance, &event.generation_id))
                    .collect::<BTreeSet<_>>()
                    .len()
                    >= GENERATION_LIMIT)
        {
            self.reject(&payload);
            return;
        }
        match source {
            REQUEST | RESPONSE => {
                let Ok(event) = serde_json::from_value::<Event>(payload.clone()) else {
                    self.reject(&payload);
                    return;
                };
                if !valid(&event)
                    || (source == REQUEST) != (event.phase == "request")
                    || (source == RESPONSE && event.phase != "success")
                {
                    self.reject(&payload);
                    return;
                }
                let key = (event.producer_instance.clone(), event.sequence);
                if self.foreign_sequences.contains(&key)
                    || self
                        .foreign_generations
                        .contains(&(event.producer_instance.clone(), event.generation_id.clone()))
                {
                    self.reject(&payload);
                    return;
                }
                if self
                    .events
                    .get(&key)
                    .is_some_and(|previous| previous != &event)
                    || self.events.values().any(|previous| {
                        previous.producer_instance == event.producer_instance
                            && previous.generation_id == event.generation_id
                            && previous.phase == event.phase
                            && previous != &event
                    })
                {
                    // Sequence reuse rejects both implicated generations, not
                    // the next unrelated generation from this capture.
                    if let Some(previous) = self.events.get(&key) {
                        let previous = json!({
                            "producer_instance": previous.producer_instance,
                            "generation_id": previous.generation_id,
                        });
                        self.reject(&previous);
                    }
                    self.reject(&payload);
                    if self.rejections_sealed {
                        return;
                    }
                }
                self.events.insert(key, event);
            }
            UPDATE => {
                if payload.pointer("/output/updated") != Some(&json!(true)) {
                    self.reject(&payload);
                    return;
                }
                let Some(input) = payload.get("input") else {
                    self.invalid = true;
                    return;
                };
                let Some(entry) = input["entry_id"].as_str() else {
                    self.invalid = true;
                    return;
                };
                // Callbacks arrive in no fixed order: an update may precede its
                // response. delivery_for checks the pair exists when it reads.
                self.updates
                    .entry(entry.into())
                    .or_default()
                    .push(input.clone());
            }
            APPEND => {
                if payload.pointer("/input/session_id").and_then(Value::as_str)
                    == Some(&self.session)
                {
                    self.appends.push(payload["input"].clone());
                }
            }
            HOOK => {
                if payload.pointer("/input/session_id").and_then(Value::as_str)
                    == Some(&self.session)
                {
                    self.hooks.push(payload);
                }
            }
            RECEIPT => {
                let Ok(receipt) = serde_json::from_value::<MemoryReceipt>(payload.clone()) else {
                    self.invalid = true;
                    return;
                };
                if receipt.version != 1
                    || !uuid4(&receipt.receipt_id)
                    || !uuid4(&receipt.producer_instance)
                    || receipt.turn_id.is_empty()
                    || receipt.turn_id.len() > 256
                    || receipt.observer_id.is_empty()
                    || receipt.observer_id.len() > 256
                    || !digest(&receipt.output_sha256)
                    || receipt.appends.is_empty()
                    || receipt.appends.len() > 2048
                    || receipt.appends.iter().enumerate().any(|(ordinal, append)| {
                        append.append_ordinal != ordinal || !digest(&append.message_sha256)
                    })
                {
                    self.invalid = true;
                    return;
                }
                if let Some(previous) = self.receipts.get(&receipt.receipt_id) {
                    if previous != &receipt {
                        self.rejected_receipts.insert(receipt.receipt_id.clone());
                    }
                    return;
                }
                self.receipts.insert(receipt.receipt_id.clone(), receipt);
            }
            NOTICE => {
                if let Some(previous) = self.notices.iter().find(|previous| {
                    previous["session_id"] == payload["session_id"]
                        && previous["entry_id"] == payload["entry_id"]
                }) {
                    if previous != &payload {
                        self.reject(&payload);
                    }
                } else {
                    self.notices.push(payload);
                }
            }
            _ => self.invalid = true,
        }
    }

    // The producer certifies private ledger semantics; the consumer independently
    // binds the same entry/body to an authenticated Memory invocation receipt.
    // Callbacks arrive in no fixed order, so hook, receipt, notice and request
    // must each exist and match; which arrived first proves nothing.
    fn source_receipt(&self, slot: &SourceSlot, notice: &Value, generation: Option<&str>) -> bool {
        if !valid_slot(slot) || notice["entry_id"] != slot.entry_id {
            return false;
        }
        let Some(message) = notice.pointer("/custom/data/message") else {
            return false;
        };
        if message_hash(message).as_deref() != Some(slot.message_sha256.as_str()) {
            return false;
        }
        let Some((head, _)) = slot
            .entry_id
            .strip_prefix("e_")
            .and_then(|id| id.rsplit_once("_notice_"))
        else {
            return false;
        };
        let Some((turn, step)) = head.rsplit_once('_') else {
            return false;
        };
        let Ok(step) = step.parse::<u64>() else {
            return false;
        };
        let Some(event) = generation.and_then(|generation| {
            self.events
                .values()
                .find(|event| event.phase == "request" && event.generation_id == generation)
        }) else {
            return false;
        };
        self.receipts.values().any(|receipt| {
            !self.rejected_receipts.contains(&receipt.receipt_id)
                && receipt.producer_instance == event.producer_instance
                && receipt.session_id == self.session
                && receipt.turn_id == turn
                && receipt.step == step
                && receipt.observer_id == slot.observer_id
                && receipt
                    .appends
                    .get(slot.append_ordinal)
                    .is_some_and(|append| {
                        append.append_ordinal == slot.append_ordinal
                            && append.message_sha256 == slot.message_sha256
                    })
                && self.hooks.iter().any(|hook| {
                    hook.pointer("/input/session_id").and_then(Value::as_str)
                        == Some(self.session.as_str())
                        && hook.pointer("/input/turn_id").and_then(Value::as_str) == Some(turn)
                        && hook.pointer("/input/step").and_then(Value::as_u64) == Some(step)
                        && hook
                            .pointer("/output/annotations/memory_bank")
                            .and_then(Value::as_str)
                            == Some(self.bank.as_str())
                        && hook.get("output").and_then(message_hash).as_deref()
                            == Some(receipt.output_sha256.as_str())
                        && hook
                            .pointer("/output/mutations/append_messages")
                            .and_then(Value::as_array)
                            .and_then(|messages| messages.get(slot.append_ordinal))
                            .is_some_and(|candidate| {
                                candidate["role"] == message["role"]
                                    && candidate["content"] == message["content"]
                            })
                })
        })
    }

    fn slot_delivery(&self, event: &Event, notice: &Value) -> Option<bool> {
        if !valid(event) || event.version != 2 {
            return None;
        }
        if !event
            .sources
            .iter()
            .chain(
                event
                    .summary
                    .iter()
                    .flat_map(|summary| summary.inputs.iter()),
            )
            .all(|slot| {
                self.notices
                    .iter()
                    .any(|notice| self.source_receipt(slot, notice, Some(&event.generation_id)))
            })
        {
            return None;
        }
        let id = notice["entry_id"].as_str()?;
        let message = notice.pointer("/custom/data/message")?;
        let source = event.sources.iter().find(|slot| slot.entry_id == id);
        let summary = event.summary.as_ref();
        let input =
            summary.and_then(|summary| summary.inputs.iter().find(|slot| slot.entry_id == id));
        if source.is_none() && input.is_none() {
            // Complete v2 absence is an objective loss only with an independently
            // authenticated Memory hook receipt; a missing receipt stays unknown.
            let known = self.receipts.values().any(|receipt| {
                receipt.appends.iter().any(|append| {
                    self.source_receipt(
                        &SourceSlot {
                            message_index: 0,
                            entry_id: id.into(),
                            message_sha256: append.message_sha256.clone(),
                            observer_id: receipt.observer_id.clone(),
                            append_ordinal: append.append_ordinal,
                            source: "replay_memory".into(),
                        },
                        notice,
                        Some(&event.generation_id),
                    )
                })
            });
            return known.then_some(false);
        }
        let slot = source.or(input)?;
        if !self.source_receipt(slot, notice, Some(&event.generation_id)) {
            return None;
        }
        let expected: Vec<Line> = message["content"]
            .as_array()?
            .iter()
            .enumerate()
            .flat_map(|(block, value)| {
                value["text"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .enumerate()
                    .filter(|(_, text)| !text.trim().is_empty())
                    .map(move |(line, text)| Line {
                        role: "user".into(),
                        location: vec![slot.message_index, block],
                        line,
                        sha256: line_hash(text),
                    })
            })
            .collect();
        if expected.is_empty() {
            return None;
        }
        if source.is_some() {
            return Some(expected.iter().all(|line| event.lines.contains(line)));
        }
        let summary = summary?;
        // All input receipts must be known, not only the fact being scored.
        if !summary.inputs.iter().all(|input| {
            self.notices
                .iter()
                .any(|notice| self.source_receipt(input, notice, Some(&event.generation_id)))
        }) {
            return None;
        }
        // These byte spans and digests are authenticated producer certificates:
        // the audited renderer validates UTF8 boundaries and both exact hashes
        // before dispatch. A consumer must not search a base-prefix collision or
        // accept a late APPEND as a replacement certificate. Shape checks here
        // do not independently re-hash the private final prompt.
        Some(summary.lines.windows(expected.len()).any(|window| {
            window
                .iter()
                .zip(&expected)
                .all(|(actual, expected)| actual.sha256 == expected.sha256)
                && window
                    .windows(2)
                    .all(|pair| pair[0].line.checked_add(1) == Some(pair[1].line))
        }))
    }

    /// Every nonempty assistant entry contributes; do not silently select a final step.
    /// The latest authenticated outbound update supersedes stale append origins.
    pub fn delivery(&self, transcript: &Value) -> Option<bool> {
        self.delivery_for(transcript, None)
    }

    pub fn delivery_for(
        &self,
        transcript: &Value,
        required: Option<&[Vec<String>]>,
    ) -> Option<bool> {
        let session = self.session.as_str();
        let bank = self.bank.as_str();
        if !self.ready || self.invalid {
            return None;
        }
        let entries = transcript["messages"].as_array()?;
        let mut credited = false;
        let mut delivered = true;
        let turns: Vec<_> = entries
            .iter()
            .filter_map(|e| e.pointer("/origin/turn_id").and_then(Value::as_str))
            .fold(Vec::new(), |mut turns, turn| {
                if !turns.contains(&turn) {
                    turns.push(turn);
                }
                turns
            });
        for entry in entries {
            if entry.pointer("/message/role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let content = entry.pointer("/message/content")?;
            if !content.as_array()?.iter().any(|block| {
                block["type"] == "text"
                    && block["text"].as_str().is_some_and(|s| !s.trim().is_empty())
            }) {
                continue;
            }
            credited = true;
            let target = entry["entry_id"].as_str()?;
            if entries
                .iter()
                .filter(|candidate| candidate["entry_id"] == target)
                .count()
                != 1
            {
                return None;
            }
            // Stamped updates arrive in no fixed order: the producer's dispatch
            // index picks the latest. One that no event orders fails closed.
            let mut latest: Option<(u64, &Value)> = None;
            for update in self.updates.get(target)? {
                let generation = update_generation(update)?;
                let dispatch = self
                    .events
                    .values()
                    .filter(|e| e.generation_id == generation)
                    .map(|e| e.dispatch_index)
                    .max()?;
                if latest.is_none_or(|(newest, _)| dispatch > newest) {
                    latest = Some((dispatch, update));
                }
            }
            let (_, update) = latest?;
            let generation = update_generation(update)?;
            if update["session_id"] != session || &update["content"] != content {
                return None;
            }
            if self.rejected_entries.contains(target)
                || self
                    .rejected_updates
                    .contains(&(target.into(), generation.into()))
            {
                return None;
            }
            let requests: Vec<_> = self
                .events
                .values()
                .filter(|e| e.generation_id == generation && e.phase == "request")
                .collect();
            let terminals: Vec<_> = self
                .events
                .values()
                .filter(|e| e.generation_id == generation && e.phase != "request")
                .collect();
            if requests.len() != 1 || terminals.len() != 1 {
                return None;
            }
            let request = requests[0];
            if self
                .rejected_generations
                .contains(&(request.producer_instance.clone(), generation.into()))
            {
                return None;
            }
            if self.events.values().any(|later| {
                later.session_id == session
                    && later.assistant_entry_id == target
                    && later.dispatch_index > request.dispatch_index
            }) {
                return None;
            }
            if request.version == 2 && !valid_origins(request) {
                return None;
            }
            if !paired(request, terminals[0])
                || request.session_id != session
                || request.assistant_entry_id != target
                || entry.pointer("/origin/turn_id").and_then(Value::as_str)
                    != Some(request.turn_id.as_str())
                || update.pointer("/origin/turn_id").and_then(Value::as_str)
                    != Some(request.turn_id.as_str())
            {
                return None;
            }
            let exchange = turns.iter().position(|turn| *turn == request.turn_id)?;
            let needed = required.and_then(|required| required.get(exchange));
            let mut wanted = BTreeSet::new();
            // Every parsed own-bank fact must also have an independent callback.
            for candidate in entries
                .iter()
                .take_while(|candidate| candidate["entry_id"] != target)
            {
                if candidate
                    .pointer("/custom/data/text")
                    .and_then(Value::as_str)
                    .is_some_and(|body| body.starts_with(&format!("<memory bank=\"{bank}\">")))
                    && !self.notices.iter().any(|notice| {
                        notice["entry_id"] == candidate["entry_id"]
                            && notice.get("custom") == candidate.get("custom")
                    })
                {
                    return None;
                }
            }
            for notice in &self.notices {
                if notice["session_id"] != session {
                    continue;
                }
                if notice
                    .pointer("/custom/custom_type")
                    .and_then(Value::as_str)
                    != Some("model_notice")
                    || notice.pointer("/custom/data/kind").and_then(Value::as_str) != Some("hook")
                {
                    continue;
                }
                let id = notice["entry_id"].as_str()?;
                let (head, _) = id
                    .strip_prefix("e_")
                    .and_then(|id| id.rsplit_once("_notice_"))?;
                let (turn, step) = head.rsplit_once('_')?;
                let step = step.parse::<u64>().ok()?;
                if turn == request.turn_id && step > request.step {
                    continue;
                }
                if notice
                    .pointer("/custom/custom_type")
                    .and_then(Value::as_str)
                    != Some("model_notice")
                    || notice.pointer("/custom/data/kind").and_then(Value::as_str) != Some("hook")
                {
                    continue;
                }
                let Some(body) = notice.pointer("/custom/data/text").and_then(Value::as_str) else {
                    continue;
                };
                if !body.starts_with(&format!("<memory bank=\"{bank}\">\n")) {
                    continue;
                }
                // The independently observed entry must still have an unambiguous
                // place before this credited assistant in the captured transcript.
                let position = entries.iter().position(|e| e["entry_id"] == id)?;
                let assistant = entries.iter().position(|e| e["entry_id"] == target)?;
                if position >= assistant {
                    continue;
                }
                if self.rejected_notices.contains(id) {
                    return None;
                }
                if entries[position].get("custom") != notice.get("custom") {
                    return None;
                }
                // A session-manager notice only proves persistence, not Memory
                // authorship. Bind the exact replayed message to independently
                // observed Memory output and the matching bank/session/turn/step.
                let message = notice.pointer("/custom/data/message")?;
                let joined = message["content"]
                    .as_array()?
                    .iter()
                    .map(|block| block["text"].as_str())
                    .collect::<Option<Vec<_>>>()?
                    .join("\n");
                if request.version == 2 && joined != body {
                    return None;
                }
                let legitimate = self.hooks.iter().any(|hook| {
                    hook.pointer("/input/session_id").and_then(Value::as_str) == Some(session)
                        && hook.pointer("/input/turn_id").and_then(Value::as_str) == Some(turn)
                        && hook.pointer("/input/step").and_then(Value::as_u64) == Some(step)
                        && hook
                            .pointer("/output/annotations/memory_bank")
                            .and_then(Value::as_str)
                            == Some(bank)
                        && hook
                            .pointer("/output/mutations/append_messages")
                            .and_then(Value::as_array)
                            .is_some_and(|messages| {
                                messages.iter().any(|m| {
                                    m["role"] == message["role"]
                                        && m["content"] == message["content"]
                                })
                            })
                });
                if !legitimate {
                    return None;
                }
                let mut fact_lines = Vec::new();
                for line in body.lines().skip(1) {
                    if line.starts_with("Standing instructions —") || line.starts_with("</memory>")
                    {
                        break;
                    }
                    if let Some(fact) = line.strip_prefix("- ") {
                        let undated = fact
                            .rsplit_once(" (")
                            .filter(|(_, date)| date.len() == 11 && date.ends_with(')'))
                            .map_or(fact, |(text, _)| text);
                        // Never require the entire historical Memory bank on every
                        // generation: only this turn's deliveries or case-needed facts.
                        if turn == request.turn_id
                            || needed.is_some_and(|facts| facts.iter().any(|text| text == undated))
                        {
                            fact_lines.push(line_hash(line));
                        }
                    }
                }
                if fact_lines.is_empty() {
                    continue;
                }
                if request.version != 2 {
                    let content = message["content"].as_array()?;
                    // Only a complete whole-message match establishes ambiguity;
                    // even a unique numeric location cannot certify authorship.
                    let locations: Vec<_> = (0..request.message_count)
                        .filter(|index| {
                            content.iter().enumerate().all(|(block, value)| {
                                value["type"] == "text"
                                    && value["text"].as_str().is_some_and(|text| {
                                        text.lines()
                                            .enumerate()
                                            .filter(|(_, line)| !line.trim().is_empty())
                                            .all(|(line, text)| {
                                                request.lines.iter().any(|w| {
                                                    w.role == "user"
                                                        && w.location == [*index, block]
                                                        && w.line == line
                                                        && w.sha256 == line_hash(text)
                                                })
                                            })
                                    })
                            })
                        })
                        .collect();
                    if locations.len() > 1 {
                        return None;
                    }
                    for hash in fact_lines {
                        wanted.insert(hash.clone());
                        let matching_notice = locations.first().is_some_and(|index| {
                            request.lines.iter().any(|w| {
                                w.role == "user"
                                    && w.location.first() == Some(index)
                                    && w.sha256 == hash
                            })
                        });
                        let matching_system = request.lines.iter().any(|w| {
                            w.role == "system" && w.location.is_empty() && w.sha256 == hash
                        });
                        if matching_notice || matching_system {
                            return None;
                        }
                        delivered = false;
                    }
                    continue;
                }
                let delivery = self.slot_delivery(request, notice)?;
                for hash in fact_lines {
                    wanted.insert(hash);
                }
                delivered &= delivery;
            }
            if request.version != 2 && delivered {
                return None;
            }
            if needed.is_some_and(|needed| {
                needed.iter().any(|fact| {
                    !wanted.iter().any(|hash| {
                        self.notices
                            .iter()
                            .filter_map(|notice| {
                                notice.pointer("/custom/data/text").and_then(Value::as_str)
                            })
                            .any(|body| {
                                body.lines().any(|line| {
                                    line_hash(line) == *hash
                                        && line.strip_prefix("- ").is_some_and(|line| {
                                            line == fact
                                                || line.strip_prefix(fact).is_some_and(|suffix| {
                                                    suffix.starts_with(" (")
                                                        && suffix.ends_with(')')
                                                })
                                        })
                                })
                            })
                    })
                })
            }) {
                delivered = false;
            }
        }
        credited.then_some(delivered)
    }
}

/// Holds actual SDK handles: pending/refused/suspended observers never authenticate.
pub struct Capture {
    data: Arc<Mutex<Captured>>,
    bindings: Arc<Mutex<Vec<(String, ObserverHandle)>>>,
    functions: Vec<iii_sdk::runtime::FunctionRef>,
}

async fn owner(client: &IIIClient, source: &str) -> Result<String> {
    let namespace = client.namespace().unwrap_or_else(|| "default".into());
    let listed: Value = client
        .call(
            iii_sdk::CallRequest::new(
                "engine::functions::list",
                json!({"namespace":namespace,"include_internal":true}),
            )
            .namespace("default"),
        )
        .await?;
    let rows = listed["functions"]
        .as_array()
        .context("function inventory unavailable")?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| row["function_id"] == source && row["namespace"] == namespace)
        .collect();
    if matches.len() != 1 {
        bail!(
            "{source}: {} registrations in namespace {namespace}, expected one",
            matches.len()
        );
    }
    let name = matches[0]["worker_name"]
        .as_str()
        .context("missing owner name")?;
    let workers: Value = client
        .call(iii_sdk::CallRequest::new("engine::workers::list", json!({})).namespace("default"))
        .await?;
    let matches: Vec<_> = workers["workers"]
        .as_array()
        .context("worker inventory unavailable")?
        .iter()
        .filter(|worker| {
            worker["name"] == name
                && worker["namespace"] == namespace
                && worker["status"] == "connected"
        })
        .collect();
    if matches.len() != 1 {
        bail!(
            "{source}: owner {name} has {} connected workers, expected one",
            matches.len()
        );
    }
    Ok(matches[0]["id"]
        .as_str()
        .context("missing owner id")?
        .to_string())
}

impl Capture {
    /// Owners are resolved once, here, and the collector authenticates each
    /// callback against them without awaiting. The SDK runs every callback in
    /// its own task, so callbacks arrive in no fixed order; `Captured` checks
    /// that each piece of evidence exists, never which one arrived first.
    pub async fn bind(context: &E2eContext, run_id: &str) -> Result<Self> {
        let client = context.client();
        let namespace = client.namespace().unwrap_or_else(|| "default".into());
        let mut capture = Self {
            data: Arc::new(Mutex::new(Captured {
                session: format!("e2e_{run_id}"),
                bank: super::super::cases::bank(run_id),
                ..Captured::default()
            })),
            bindings: Arc::new(Mutex::new(Vec::new())),
            functions: Vec::new(),
        };
        let harness_owner = owner(client, REQUEST).await?;
        for (index, source) in SOURCES.into_iter().enumerate() {
            // Resolve the emitting connection; private does not authenticate readers.
            let source_owner = owner(client, source).await?;
            // Auto activations are called by the ENGINE, not by the source
            // worker (engine observer/dispatch.rs:807). Its projected caller
            // is the routed writer: the Harness for the calls it makes.
            let expected_caller = if matches!(source, UPDATE | HOOK | APPEND) {
                harness_owner.clone()
            } else {
                source_owner
            };
            let target = format!(
                "e2e::memory::generation::{run_id}::{index}::{}",
                uuid::Uuid::new_v4()
            );
            let data = capture.data.clone();
            let bindings = capture.bindings.clone();
            let collector = client.clone();
            let namespace = namespace.clone();
            let function = client.register_function(
                target.clone(),
                RegisterFunction::new(move |payload: Value| {
                    let ctx = iii_sdk::current_invocation();
                    let caller = ctx.as_ref().and_then(|ctx| ctx.caller.as_ref());
                    let activation = ctx.as_ref().and_then(|ctx| ctx.activation.as_ref());
                    let authenticated_source = collector.get_connection_state()
                        == iii_sdk::runtime::IIIConnectionState::Connected
                        && activation.is_some_and(|activation| {
                            activation.activation_id.is_some()
                                && payload["observer_id"] == activation.observer_id
                                && payload["activation_id"].as_str()
                                    == activation.activation_id.as_deref()
                                && bindings.lock().unwrap().iter().any(|(id, handle)| {
                                    id == source
                                        && handle.id() == activation.observer_id
                                        && handle.state() == ObserverState::Active
                                })
                        })
                        && caller.is_some_and(|caller| {
                            caller.worker_id.is_none()
                                && caller.worker_name.as_deref() == Some("iii-engine")
                                && caller.function_id.as_deref() == Some(source)
                        })
                        && payload["function_id"] == source
                        && payload["namespace"] == namespace.as_str()
                        && payload.pointer("/caller/worker_id").and_then(Value::as_str)
                            == Some(expected_caller.as_str());
                    let authentic = authenticated_source && payload["status"] == "success";
                    // Never trust the declared session of an unauthenticated
                    // caller/activation. A trusted source reporting failure
                    // may reject only attributable own proof.
                    let observed = if matches!(source, UPDATE | HOOK | APPEND) {
                        let mut payload = payload;
                        if let Some(object) = payload.as_object_mut() {
                            object.remove("source_observers");
                            object.remove("source_observer");
                        }
                        payload
                    } else {
                        payload["output"].clone()
                    };
                    data.lock().unwrap().record_callback(
                        source,
                        observed,
                        authenticated_source,
                        authentic,
                    );
                    Ok(json!({"accepted": authentic}))
                })
                .description("Internal authenticated final-SDK memory witness collector."),
            );
            capture.functions.push(function);
            let source_spec = iii_sdk::Source::function(source).capture([
                iii_sdk::Capture::Input,
                iii_sdk::Capture::Output,
                iii_sdk::Capture::Caller,
                iii_sdk::Capture::Metadata,
            ]);
            let handle = client.observe(source_spec, target);
            // Retain before ready so the first activation can only reference a real handle.
            let ready = handle.clone();
            capture
                .bindings
                .lock()
                .unwrap()
                .push((source.into(), handle));
            tokio::time::timeout(Duration::from_secs(10), ready.ready())
                .await
                .with_context(|| format!("{source}: observer not ready in 10s"))??;
        }
        capture.data.lock().unwrap().ready = true;
        Ok(capture)
    }

    /// Shared runtime/adapter seam; it never consumes pending callbacks.
    pub fn populate_snapshot(&self, snapshot: &mut super::super::score::Snapshot) {
        let data = self.snapshot();
        snapshot.generation_messages_unavailable = !data.available();
        snapshot.generations = Some(data);
    }

    pub fn snapshot(&self) -> Captured {
        let mut data = self.data.lock().unwrap().clone();
        data.ready &= self
            .bindings
            .lock()
            .unwrap()
            .iter()
            .all(|(_, handle)| handle.state() == ObserverState::Active);
        data
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.data.lock().unwrap().ready = false;
        for (_, handle) in self.bindings.lock().unwrap().drain(..) {
            crate::observe::release(handle);
        }
        for function in self.functions.drain(..) {
            function.unregister();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::sync::{mpsc, oneshot};
    use tokio_tungstenite::tungstenite::Message;

    const FACT: &str = "Atlas deploys with ember. (2026-10-23)";
    const INSTANCE: &str = "d9f17977-49b0-4eb9-bd79-187905072023";
    const A: &str = "fa04ee0b-2494-4551-9251-3f6c7fb63a33";
    const B: &str = "b21fe0a9-5263-4fb4-a955-df7568d91fc2";

    fn event(generation: &str, sequence: u64, fact: bool) -> Event {
        Event {
            version: 1,
            sources: Vec::new(),
            origins_complete: false,
            summary: None,
            producer_instance: INSTANCE.into(),
            generation_id: generation.into(),
            assistant_entry_id: "e_t_0_assistant".into(),
            session_id: "e2e_control".into(),
            turn_id: "t".into(),
            step: 0,
            dispatch_index: sequence / 2,
            sequence,
            phase: "request".into(),
            complete: true,
            message_count: 1,
            text_bytes: 100,
            lines: if fact {
                memory_body()
                    .lines()
                    .enumerate()
                    .map(|(line, text)| Line {
                        role: "user".into(),
                        location: vec![0, 0],
                        line,
                        sha256: line_hash(text),
                    })
                    .collect()
            } else {
                vec![Line {
                    role: "user".into(),
                    location: vec![0, 0],
                    line: 0,
                    sha256: line_hash("ordinary prompt without Memory"),
                }]
            },
        }
    }

    // Native producer serializer output, copied byte-for-byte once; not a consumer oracle.
    // Producer source SHA256 a935cb58243a122ccea52de62ba903687741b3d336c34b5fdd969e9d96966ed2.
    const RAW_SLOT: &str = include_str!("test_data/generation_v2.json");
    const RAW_SUMMARY: &str = include_str!("test_data/generation_summary_v2.json");
    const RAW_CONTEXT: &str = include_str!("test_data/generation_context_v2.json");

    fn raw_notice(summary: bool) -> Value {
        let text = if summary {
            "synthetic-turn-fact\nother fact"
        } else {
            "synthetic-turn-fact"
        };
        json!({"session_id":if summary {"synthetic-session-origin_summary"} else {"synthetic-session-origin_both"},
            "entry_id":"e_synthetic-turn_7_notice_0","custom":{"custom_type":"model_notice",
            "data":{"kind":"hook","text":text,"message":{"role":"user","timestamp":0,"content":[{"type":"text","text":text}]}}}})
    }

    fn raw_hook(summary: bool) -> Value {
        let notice = raw_notice(summary);
        json!({"input":{"session_id":notice["session_id"],"turn_id":"synthetic-turn","step":7},
            "source_observers":["source-observer"],"output":{"annotations":{"memory_bank":"e2e-raw"},
            "mutations":{"append_messages":[notice["custom"]["data"]["message"].clone()]}}})
    }

    // The genuine fixtures deliberately contain plain text at step 7, not a
    // step-0 Memory block. A successful slot check must never be reported as a
    // positive named delivery gate: delivery(None) has no required facts here.
    #[test]
    fn raw_fixture_missing_named_gates_regression() {
        let request: Event = serde_json::from_str(RAW_SLOT).unwrap();
        let mut captured = raw_data(&request);
        captured.record(UPDATE,json!({"input":{"session_id":request.session_id,"entry_id":request.assistant_entry_id,"content":[{"type":"text","text":"done"}],"origin":{"generation_id":request.generation_id,"turn_id":request.turn_id}},"output":{"updated":true}}));
        let transcript = json!({"messages":[raw_notice(false),{"entry_id":request.assistant_entry_id,"origin":{"turn_id":request.turn_id,"memory_bank":"e2e-raw","memory_recalled":0},"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}]});
        assert_raw_fixture_missing_named_gates(captured, &transcript);
    }

    fn assert_raw_fixture_missing_named_gates(captured: Captured, transcript: &Value) {
        use crate::report::{gate_completion, CompletionState, CriterionReport};
        use crate::scenarios::memory::{
            cases::{Case, Check, Criterion, Seed, CASES},
            score,
        };
        let case = Case {
            memories: &[Seed {
                text: "synthetic-turn-fact",
                group: "raw_fact",
                needed_on: &[0],
                related_on: &[],
                entities: &[],
            }],
            followups: &[],
            rules: &[],
            changes: &[],
            criteria: &[
                Criterion {
                    id: "strict_delivery",
                    weight: 50,
                    gate: true,
                    description: "StrictDelivery",
                    label: "StrictDelivery",
                    check: Check::StrictDelivery,
                },
                Criterion {
                    id: "needed_injected",
                    weight: 50,
                    gate: true,
                    description: "NeededInjected",
                    label: "NeededInjected",
                    check: Check::NeededInjected { exchange: None },
                },
            ],
            ..CASES[29]
        };
        let required = vec![vec!["synthetic-turn-fact".to_owned()]];
        assert_eq!(
            captured.delivery_for(transcript, Some(&required)),
            Some(false)
        );
        let snapshot = score::Snapshot {
            generation_messages_unavailable: !captured.available(),
            generations: Some(captured),
            stored: vec![Some("synthetic-turn-fact".into())],
            ..Default::default()
        };
        let evidence = score::evidence(&case, "raw", transcript, snapshot);
        assert_eq!(evidence.observed.exchanges.len(), 1);
        assert!(evidence.observed.blocks.is_empty());
        assert_eq!(
            evidence.strict.as_ref().unwrap().generation_delivery,
            Some(false)
        );
        let evaluation = score::evaluate(&case, &evidence);
        assert!(evaluation.infrastructure_error.is_none());
        let reports: Vec<_> = score::criteria(&case)
            .iter()
            .map(|criterion| {
                let award = evaluation
                    .awards
                    .iter()
                    .find(|award| award.id == criterion.id)
                    .unwrap();
                assert_eq!(award.awarded, Some(0), "{}: {}", criterion.id, award.reason);
                CriterionReport {
                    id: criterion.id.into(),
                    description: None,
                    label: None,
                    possible: criterion.weight,
                    awarded: award.awarded,
                    reason: award.reason.clone(),
                    gate: criterion.gate,
                    gate_minimum: criterion.gate_minimum,
                }
            })
            .collect();
        assert_eq!(gate_completion(&reports), CompletionState::TaskIncomplete);
    }

    fn raw_data(request: &Event) -> Captured {
        let summary = request.summary.is_some();
        let mut data = Captured {
            ready: true,
            session: request.session_id.clone(),
            bank: "e2e-raw".into(),
            ..Captured::default()
        };
        data.record(HOOK, raw_hook(summary));
        data.record(
            RECEIPT,
            test_receipt(request, &raw_hook(summary), "source-observer"),
        );
        data.record(NOTICE, raw_notice(summary));
        data.record(REQUEST, serde_json::to_value(request).unwrap());
        data.record(RESPONSE, terminal(request));
        data
    }

    #[test]
    fn native_producer_raw_slot_and_summary_contract_and_adversaries() {
        for (wire, expected_sequence, expected_dispatch) in
            [(RAW_SLOT, 51, 63), (RAW_SUMMARY, 42, 56)]
        {
            let request: Event = serde_json::from_str(wire).unwrap();
            assert_eq!(request.version, 2);
            assert_eq!(request.sequence, expected_sequence);
            assert_eq!(request.dispatch_index, expected_dispatch);
            assert_eq!(request.turn_id, "synthetic-turn");
            assert!(valid(&request));
            let data = raw_data(&request);
            assert_eq!(
                data.slot_delivery(&request, &raw_notice(request.summary.is_some())),
                Some(true)
            );
            let mut missing = request.clone();
            missing.sources.clear();
            missing.summary = None;
            assert_eq!(
                data.slot_delivery(&missing, &raw_notice(request.summary.is_some())),
                Some(false)
            );
            for field in [
                "version",
                "complete",
                "origins_complete",
                "sources",
                "summary",
            ] {
                let mut malformed: Value = serde_json::from_str(wire).unwrap();
                malformed.as_object_mut().unwrap().remove(field);
                assert!(
                    serde_json::from_value::<Event>(malformed).is_err(),
                    "missing {field}"
                );
            }
        }
        let request: Event = serde_json::from_str(RAW_SUMMARY).unwrap();
        let context: Value = serde_json::from_str(RAW_CONTEXT).unwrap();
        let system = context["output"]["system_prompt"].as_str().unwrap();
        let summary = request.summary.as_ref().unwrap();
        assert_eq!(summary.span.byte_start, 56);
        assert_eq!(summary.span.byte_end, 86);
        assert_eq!(&system[56..86], "synthetic-turn-fact\nother fact");
        assert_eq!(
            format!("{:x}", Sha256::digest(system.as_bytes())),
            summary.span.system_sha256
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&system.as_bytes()[56..86])),
            summary.span.summary_sha256
        );
        assert_eq!(
            summary
                .lines
                .iter()
                .map(|line| line.line)
                .collect::<Vec<_>>(),
            vec![5, 6]
        );
        assert_eq!(
            summary.lines[0].sha256,
            "3ce6d0c89822ed2d567883921665e32e1d6c8241643d542caa1d99fa0bc1ac14"
        );
        let data = raw_data(&request);
        for mutation in 0..10 {
            let mut bad = request.clone();
            let summary = bad.summary.as_mut().unwrap();
            match mutation {
                0 => summary.span.byte_start = summary.span.byte_end,
                1 => summary.span.byte_end = usize::MAX,
                2 => summary.lines.reverse(),
                3 => summary.lines[1].line = 8,
                4 => summary.inputs[0].observer_id = "foreign".into(),
                5 => summary.inputs[0].entry_id = "foreign".into(),
                6 => summary.inputs[0].message_sha256 = "0".repeat(64),
                7 => summary.span.system_sha256 = "invalid".into(),
                8 => summary.span.summary_sha256 = "invalid".into(),
                9 => {
                    summary.lines[0].line = 0;
                    summary.lines[1].line = 1;
                    bad.origins_complete = false;
                }
                _ => unreachable!(),
            }
            assert!(
                !valid(&bad) || data.slot_delivery(&bad, &raw_notice(true)) != Some(true),
                "mutation {mutation}"
            );
        }
        for field in ["byte_start", "byte_end", "summary_sha256", "system_sha256"] {
            let mut wire: Value = serde_json::from_str(RAW_SUMMARY).unwrap();
            wire["summary"]["span"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(serde_json::from_value::<Event>(wire).is_err());
        }
        let mut foreign = data.clone();
        foreign.hooks[0]["output"]["annotations"]["memory_bank"] = json!("foreign");
        assert_eq!(foreign.slot_delivery(&request, &raw_notice(true)), None);
        let mut late = request;
        late.summary = None;
        let mut late_data = data;
        late_data.record(APPEND,json!({"input":{"session_id":late.session_id,"entry_id":"late","custom":{"custom_type":"compaction","data":{"summary":"synthetic-turn-fact\nother fact"}}},"output":{"entry_id":"late"}}));
        assert_eq!(
            late_data.slot_delivery(&late, &raw_notice(true)),
            Some(false)
        );
    }

    #[test]
    fn receipt_chain_needs_every_piece_but_not_their_arrival_order() {
        let request: Event = serde_json::from_str(RAW_SUMMARY).unwrap();
        let mut late = raw_data(&request);
        late.hooks.clear();
        assert_eq!(late.slot_delivery(&request, &raw_notice(true)), None);
        // The hook arriving after its receipt, notice and request still counts.
        late.record(HOOK, raw_hook(true));
        assert_eq!(late.slot_delivery(&request, &raw_notice(true)), Some(true));
        let mut base_only = request.clone();
        base_only.summary.as_mut().unwrap().lines.clear();
        assert_ne!(
            raw_data(&request).slot_delivery(&base_only, &raw_notice(true)),
            Some(true)
        );
        let mut scattered = request.clone();
        let summary = scattered.summary.as_mut().unwrap();
        summary.lines[1].line = 8;
        scattered.lines.push(summary.lines[1].clone());
        assert_eq!(
            raw_data(&request).slot_delivery(&scattered, &raw_notice(true)),
            Some(false)
        );
    }

    fn v2_event() -> Event {
        let mut request = event(A, 41, true);
        request.version = 2;
        request.origins_complete = true;
        request.sources = vec![SourceSlot {
            message_index: 0,
            entry_id: "e_t_0_notice_0".into(),
            message_sha256: message_hash(&notice()["custom"]["data"]["message"]).unwrap(),
            observer_id: "memory-observer".into(),
            append_ordinal: 0,
            source: "replay_memory".into(),
        }];
        request
    }

    #[test]
    fn v2_exact_slot_distinguishes_identical_user_twin_and_rejects_receipt_mutations() {
        let request = v2_event();
        let mut captured = data(&request);
        captured.hooks[0]["source_observers"] = json!(["memory-observer"]);
        assert_eq!(captured.delivery(&transcript()), Some(true));
        let mut unknown = captured.clone();
        unknown.hooks.clear();
        assert_eq!(unknown.delivery(&transcript()), None);
        let mut forged = captured.clone();
        forged.receipts.clear();
        forged.hooks[0]["source_observers"] = json!([]);
        forged.hooks[0]["observer"] = json!({"id":"memory-observer"});
        forged.hooks[0]["output"]["annotations"]["actor"] = json!("memory");
        assert_eq!(forged.delivery(&transcript()), None);
        let mut twins = request.clone();
        twins.message_count = 2;
        let mut copy = twins.lines.clone();
        for line in &mut copy {
            line.location[0] = 1;
        }
        twins.lines.extend(copy);
        let mut both = data(&twins);
        both.hooks[0]["source_observers"] = json!(["memory-observer"]);
        assert_eq!(both.delivery(&transcript()), Some(true));
        // Removing only the ordinary identical slot retains the Memory receipt;
        // removing Memory and retaining only the user copy is observed absence.
        let mut retained = twins.clone();
        retained.message_count = 1;
        retained
            .lines
            .retain(|line| line.location.first() == Some(&0));
        let mut retained_data = data(&retained);
        retained_data.hooks[0]["source_observers"] = json!(["memory-observer"]);
        assert_eq!(retained_data.delivery(&transcript()), Some(true));
        let mut user_only = twins;
        user_only.sources.clear();
        user_only
            .lines
            .retain(|line| line.location.first() == Some(&1));
        let mut removed = data(&user_only);
        removed.hooks[0]["source_observers"] = json!(["memory-observer"]);
        assert_eq!(removed.delivery(&transcript()), Some(false));
        assert_named_missing_gates(removed, false);
        for mutation in 0..10 {
            let mut altered = request.clone();
            match mutation {
                0 => altered.sources[0].observer_id = "foreign".into(),
                1 => altered.sources[0].append_ordinal = 1,
                2 => altered.sources[0].entry_id = "foreign".into(),
                3 => altered.sources[0].message_sha256 = "0".repeat(64),
                4 => altered.sources.clear(),
                6 => altered.sources[0].source = "user".into(),
                7 => altered.origins_complete = false,
                8 => altered.complete = false,
                9 => altered.sources[0].message_index = usize::MAX,
                5 => {
                    altered.message_count = 2;
                    for line in &mut altered.lines {
                        line.location[0] = 1;
                    }
                }
                _ => unreachable!(),
            }
            let mut altered = data(&altered);
            altered.hooks[0]["source_observers"] = json!(["memory-observer"]);
            assert_ne!(
                altered.delivery(&transcript()),
                Some(true),
                "mutation {mutation}"
            );
        }
    }

    fn assert_native_gates(
        mut snapshot: crate::scenarios::memory::score::Snapshot,
        transcript: &Value,
        expected: Option<bool>,
    ) {
        use crate::scenarios::memory::{
            cases::{Case, Check, Criterion, Seed, CASES},
            score,
        };
        let case = Case {
            memories: &[Seed {
                text: "Atlas deploys with ember.",
                group: "deploy",
                needed_on: &[0],
                related_on: &[],
                entities: &[],
            }],
            followups: &[],
            rules: &[],
            changes: &[],
            criteria: &[
                Criterion {
                    id: "strict_delivery",
                    weight: 50,
                    gate: true,
                    description: "StrictDelivery",
                    label: "StrictDelivery",
                    check: Check::StrictDelivery,
                },
                Criterion {
                    id: "needed_injected",
                    weight: 50,
                    gate: true,
                    description: "NeededInjected",
                    label: "NeededInjected",
                    check: Check::NeededInjected { exchange: None },
                },
            ],
            ..CASES[29]
        };
        snapshot.stored = vec![Some("Atlas deploys with ember.".into())];
        let evidence = score::evidence(&case, "native", transcript, snapshot);
        assert!(!evidence.observed.blocks.is_empty());
        assert_eq!(
            evidence.strict.as_ref().unwrap().generation_delivery,
            expected
        );
        let evaluation = score::evaluate(&case, &evidence);
        if expected == Some(true) {
            assert!(
                evaluation.infrastructure_error.is_none(),
                "{:?}",
                evaluation.infrastructure_error
            );
        }
        for id in ["strict_delivery", "needed_injected"] {
            let award = evaluation
                .awards
                .iter()
                .find(|award| award.id == id)
                .unwrap();
            assert_eq!(
                award.awarded,
                Some(if expected == Some(true) { 50 } else { 0 }),
                "{id}: {}",
                award.reason
            );
        }
        let reports: Vec<_> = evaluation
            .awards
            .iter()
            .map(|award| crate::report::CriterionReport {
                id: award.id.clone(),
                description: None,
                label: None,
                possible: 50,
                awarded: award.awarded,
                reason: award.reason.clone(),
                gate: true,
                gate_minimum: None,
            })
            .collect();
        assert_eq!(
            crate::report::gate_completion(&reports),
            if expected == Some(true) {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            }
        );
        if expected == Some(true) {
            assert_ne!(
                evaluation.completion,
                Some(crate::report::CompletionState::TaskIncomplete)
            );
        }
    }

    fn assert_named_positive_gates(mut snapshot: crate::scenarios::memory::score::Snapshot) {
        use crate::scenarios::memory::{
            cases::{Case, Check, Criterion, Seed, CASES},
            score,
        };
        let case = Case {
            memories: &[Seed {
                text: "Atlas deploys with ember.",
                group: "deploy",
                needed_on: &[0],
                related_on: &[],
                entities: &[],
            }],
            followups: &[],
            rules: &[],
            changes: &[],
            criteria: &[
                Criterion {
                    id: "strict_delivery",
                    weight: 50,
                    gate: true,
                    description: "StrictDelivery",
                    label: "StrictDelivery",
                    check: Check::StrictDelivery,
                },
                Criterion {
                    id: "needed_injected",
                    weight: 50,
                    gate: true,
                    description: "NeededInjected",
                    label: "NeededInjected",
                    check: Check::NeededInjected { exchange: None },
                },
            ],
            ..CASES[29]
        };
        snapshot.prompts = vec![score::PromptRecord {
            turn_id: "t".into(),
            step: 0,
            system_prompt: "Helpful.\n\n# Memory — bank: e2e-control\nAutomatic memory.".into(),
        }];
        let evidence = score::evidence(&case, "control", &transcript(), snapshot);
        let evaluation = score::evaluate(&case, &evidence);
        assert!(evaluation.infrastructure_error.is_none());
        assert_ne!(
            evaluation.completion,
            Some(crate::report::CompletionState::TaskIncomplete)
        );
        for id in ["strict_delivery", "needed_injected"] {
            assert_eq!(
                evaluation
                    .awards
                    .iter()
                    .find(|a| a.id == id)
                    .unwrap()
                    .awarded,
                Some(50),
                "{id}: {}",
                evaluation
                    .awards
                    .iter()
                    .find(|a| a.id == id)
                    .unwrap()
                    .reason
            );
        }
    }

    #[test]
    fn v2_named_positive_and_absent_source_gates() {
        let request = v2_event();
        let mut captured = data(&request);
        captured.hooks[0]["source_observers"] = json!(["memory-observer"]);
        assert_named_positive_gates(crate::scenarios::memory::score::Snapshot {
            generations: Some(captured),
            ..Default::default()
        });
        let mut absent = request;
        absent.sources.clear();
        let mut captured = data(&absent);
        captured.hooks[0]["source_observers"] = json!(["memory-observer"]);
        assert_eq!(captured.delivery(&transcript()), Some(false));
        assert_named_missing_gates(captured, false);
    }

    #[test]
    fn v2_summary_key_is_required_and_numeric_and_source_bounds_are_checked() {
        let request = v2_event();
        let mut wire = serde_json::to_value(&request).unwrap();
        wire.as_object_mut().unwrap().remove("summary");
        assert!(serde_json::from_value::<Event>(wire).is_err());
        for path in [
            "/sources/0/message_index",
            "/sources/0/append_ordinal",
            "/text_bytes",
        ] {
            let mut wire = serde_json::to_value(&request).unwrap();
            *wire.pointer_mut(path).unwrap() = json!(-1);
            assert!(serde_json::from_value::<Event>(wire).is_err());
        }
        let mut invalid = request;
        invalid.sources[0].message_index = invalid.message_count;
        assert!(!valid(&invalid));
    }

    fn terminal(request: &Event) -> Value {
        let mut event = request.clone();
        event.phase = "success".into();
        event.sequence += 1;
        event.lines.clear();
        serde_json::to_value(event).unwrap()
    }

    fn memory_body() -> String {
        format!("<memory bank=\"e2e-control\">\nRelevant remembered memories:\n- {FACT}\n</memory>")
    }

    fn hook() -> Value {
        json!({"input":{"session_id":"e2e_control","turn_id":"t","step":0},"output":{"decision":"continue","annotations":{"memory_bank":"e2e-control"},"mutations":{"append_messages":[{"role":"user","content":[{"type":"text","text":memory_body()}]}]}}})
    }

    fn transcript() -> Value {
        json!({"messages":[
            {"entry_id":"e_t_0_notice_0","custom":{"custom_type":"model_notice","data":{"kind":"hook","text":memory_body(),"message":{"role":"user","content":[{"type":"text","text":memory_body()}]}}}},
            {"entry_id":"e_t_0_assistant","origin":{"turn_id":"t","memory_bank":"e2e-control","memory_recalled":1},"message":{"role":"assistant","content":[{"type":"text","text":"ember"}]}}
        ]})
    }

    fn notice() -> Value {
        let mut notice = transcript()["messages"][0].clone();
        notice["session_id"] = json!("e2e_control");
        notice["custom"]["data"]["message"] =
            json!({"role":"user","content":[{"type":"text","text":memory_body()}]});
        notice
    }

    fn update(generation: &str) -> Value {
        json!({"input":{"entry_id":"e_t_0_assistant","session_id":"e2e_control","content":transcript()["messages"][1]["message"]["content"],"origin":{"generation_id":generation,"turn_id":"t"}},"output":{"updated":true,"revision":1},"caller":{"worker_id":"harness-owner"}})
    }

    fn test_receipt(request: &Event, hook: &Value, observer: &str) -> Value {
        json!({"version":1,"receipt_id":uuid::Uuid::new_v4().to_string(),
            "producer_instance":request.producer_instance,"session_id":request.session_id,
            "turn_id":hook["input"]["turn_id"],"step":hook["input"]["step"],"observer_id":observer,
            "output_sha256":message_hash(&hook["output"]).unwrap(),"appends":[{"append_ordinal":0,
                "message_sha256":message_hash(&if observer == "source-observer" {raw_notice(request.summary.is_some())["custom"]["data"]["message"].clone()} else {notice()["custom"]["data"]["message"].clone()}).unwrap()}]})
    }

    fn data(request: &Event) -> Captured {
        let mut data = Captured {
            ready: true,
            session: "e2e_control".into(),
            bank: "e2e-control".into(),
            ..Captured::default()
        };
        data.record(HOOK, hook());
        data.record(RECEIPT, test_receipt(request, &hook(), "memory-observer"));
        data.record(NOTICE, notice());
        data.record(REQUEST, serde_json::to_value(request).unwrap());
        data.record(RESPONSE, terminal(request));
        data.record(UPDATE, update(&request.generation_id));
        data
    }

    #[test]
    fn producer_wire_schema_and_exact_whole_line_normalization() {
        let value = serde_json::to_value(event(A, 41, true)).unwrap();
        assert_eq!(
            serde_json::from_value::<Event>(value.clone()).unwrap(),
            event(A, 41, true)
        );
        for missing in [
            "generation_id",
            "complete",
            "lines",
            "dispatch_index",
            "assistant_entry_id",
        ] {
            let mut malformed = value.clone();
            malformed.as_object_mut().unwrap().remove(missing);
            assert!(serde_json::from_value::<Event>(malformed).is_err());
        }
        let lines: Vec<_> = " \u{2003}- Exact Case \r\n* Other\n+ Third\n- - Nested"
            .lines()
            .map(line_hash)
            .collect();
        assert_eq!(lines[0], line_hash("Exact Case"));
        assert_eq!(lines[1], line_hash("Other"));
        assert_eq!(lines[2], line_hash("Third"));
        assert_eq!(lines[3], format!("{:x}", Sha256::digest(b"- Nested")));
        assert_ne!(line_hash(FACT), line_hash("ember"));
        assert_ne!(line_hash("Exact Case"), line_hash("exact case"));
    }

    #[test]
    fn retries_and_compaction_require_current_exact_fact_not_old_append() {
        let mut captured = data(&event(A, 101, true));
        assert!(captured.available());
        assert_eq!(captured.delivery(&transcript()), None);
        let retry = event(B, 105, false);
        captured.record(REQUEST, serde_json::to_value(&retry).unwrap());
        captured.record(RESPONSE, terminal(&retry));
        captured.record(UPDATE, update(B));
        assert_eq!(captured.delivery(&transcript()), Some(false));
        // Redelivery has matching bytes, but v1 has no entry-to-slot identity.
        let redelivery = data(&event(B, 105, true));
        let mut compacted = transcript();
        compacted["messages"].as_array_mut().unwrap().insert(1,json!({"entry_id":"compaction","custom":{"custom_type":"compaction","data":{"summary":FACT}}}));
        assert!(redelivery.available());
        assert_eq!(redelivery.delivery(&compacted), None);
        let mut different = event(B, 105, true);
        different.lines[2].sha256 = line_hash("Atlas uses ember.");
        assert_eq!(data(&different).delivery(&compacted), Some(false));
        assert_eq!(
            data(&event(B, 105, false)).delivery(&compacted),
            Some(false)
        );
        let mut user = event(B, 105, true);
        user.lines[2].location = vec![0, 1];
        assert_eq!(data(&user).delivery(&transcript()), Some(false));
    }

    #[test]
    fn summary_without_authorized_location_is_unavailable_even_after_append() {
        let mut summary = event(B, 105, false);
        summary.lines = vec![Line {
            role: "system".into(),
            location: vec![],
            line: 0,
            sha256: line_hash(FACT),
        }];
        let mut captured = data(&summary);
        assert_eq!(captured.delivery(&transcript()), None);
        captured.record(APPEND, json!({"input":{"session_id":"e2e_control","entry_id":"summary","custom":{"custom_type":"compaction","data":{"summary":FACT}}}}));
        assert!(captured.available());
        assert_eq!(captured.delivery(&transcript()), None);
        captured.appends[0]["custom"]["data"]["summary"] = json!("Atlas uses ember.");
        assert_eq!(captured.delivery(&transcript()), None);
        // Prior, later, reordered and separated hash sets cannot prove the
        // compaction's system offset under the existing producer interface.
        for lines in [
            vec![FACT, "tail"],
            vec!["tail", FACT],
            vec![FACT, "gap", "tail"],
        ] {
            let mut request = event(B, 105, false);
            request.lines = lines
                .iter()
                .enumerate()
                .map(|(line, text)| Line {
                    role: "system".into(),
                    location: vec![],
                    line,
                    sha256: line_hash(text),
                })
                .collect();
            let mut prior = Captured {
                ready: true,
                session: "e2e_control".into(),
                bank: "e2e-control".into(),
                ..Captured::default()
            };
            prior.record(HOOK, hook());
            prior.record(NOTICE, notice());
            prior.record(APPEND,json!({"input":{"session_id":"e2e_control","entry_id":"summary","custom":{"custom_type":"compaction","data":{"summary":format!("{FACT}\ntail")}}}}));
            prior.record(REQUEST, serde_json::to_value(&request).unwrap());
            prior.record(RESPONSE, terminal(&request));
            prior.record(UPDATE, update(B));
            assert_eq!(prior.delivery(&transcript()), None);
        }
        let mut historical_request = event(B, 105, false);
        historical_request.version = 2;
        historical_request.origins_complete = true;
        let mut historical = data(&historical_request);
        let mut log = transcript();
        log["messages"][0]["entry_id"] = json!("e_prior_0_notice_0");
        historical.notices[0]["entry_id"] = json!("e_prior_0_notice_0");
        historical.hooks[0]["input"]["turn_id"] = json!("prior");
        for receipt in historical.receipts.values_mut() {
            receipt.turn_id = "prior".into();
        }
        assert_eq!(historical.delivery_for(&log, Some(&[vec![]])), Some(true));
        assert_eq!(
            historical.delivery_for(&log, Some(&[vec!["Atlas deploys with ember.".into()]])),
            Some(false)
        );
    }

    #[test]
    fn missing_conflicting_mixed_or_wrong_generation_evidence_fails_closed() {
        let baseline = event(A, 41, true);
        for mutation in 0..12 {
            let mut captured = data(&baseline);
            match mutation {
                0 => captured.updates.clear(),
                1 => {
                    captured.events.retain(|_, e| e.phase != "success");
                }
                2 => {
                    captured.events.retain(|_, e| e.phase != "request");
                }
                3 => {
                    captured.ready = false;
                }
                4 => {
                    captured.updates.get_mut("e_t_0_assistant").unwrap()[0]["origin"]
                        ["generation_id"] = json!(B);
                }
                5 => {
                    let mut conflict = baseline.clone();
                    conflict.lines.clear();
                    captured.record(REQUEST, serde_json::to_value(conflict).unwrap());
                }
                6 => {
                    for event in captured.events.values_mut() {
                        event.session_id = "other".into();
                    }
                }
                7 => {
                    for event in captured.events.values_mut() {
                        event.step = 1;
                    }
                }
                8 => {
                    for event in captured.events.values_mut() {
                        event.complete = false;
                    }
                }
                9 => {
                    for event in captured
                        .events
                        .values_mut()
                        .filter(|e| e.phase == "success")
                    {
                        event.producer_instance = B.into();
                    }
                }
                10 => {
                    for event in captured
                        .events
                        .values_mut()
                        .filter(|e| e.phase == "success")
                    {
                        event.phase = "cancelled".into();
                    }
                }
                11 => captured.notices.clear(),
                _ => unreachable!(),
            }
            assert_eq!(
                captured.delivery(&transcript()),
                None,
                "mutation {mutation}"
            );
        }
        let mut captured = data(&baseline);
        captured.record(REQUEST, serde_json::to_value(&baseline).unwrap());
        captured.record(RESPONSE, terminal(&baseline));
        assert!(captured.available());
        assert_eq!(captured.delivery(&transcript()), None);
        // A missing global prefix or unrelated session pair is not a missing own pair.
        let mut other = event(B, 500, false);
        other.session_id = "unrelated".into();
        captured.record(REQUEST, serde_json::to_value(other).unwrap());
        assert!(captured.available());
        assert_eq!(captured.delivery(&transcript()), None);
    }

    #[test]
    fn missing_proof_fails_strict_and_needed_gates_without_claiming_delivery_defect() {
        use crate::scenarios::memory::{cases, score, strict};
        let case = &cases::CASES[29];
        let snapshot = score::Snapshot {
            generation_messages_unavailable: true,
            ..score::Snapshot::default()
        };
        let evidence = score::evidence(case, "control", &json!({"messages":[]}), snapshot);
        assert!(!strict::judge(case, &evidence).0);
        let evaluation = score::evaluate(case, &evidence);
        assert!(evaluation
            .awards
            .iter()
            .any(|award| award.awarded == Some(0)));
        let mut defect = data(&event(A, 41, false));
        assert_eq!(defect.delivery(&transcript()), Some(false));
        defect.updates.clear();
        assert_eq!(defect.delivery(&transcript()), None);
    }

    fn assert_named_missing_gates(captured: Captured, unavailable: bool) {
        let snapshot = crate::scenarios::memory::score::Snapshot {
            generation_messages_unavailable: !captured.available(),
            generations: Some(captured),
            ..crate::scenarios::memory::score::Snapshot::default()
        };
        assert_missing_snapshot_gates(snapshot, unavailable);
    }

    fn assert_missing_snapshot_gates(
        mut snapshot: crate::scenarios::memory::score::Snapshot,
        unavailable: bool,
    ) {
        use crate::scenarios::memory::{
            cases::{Case, Check, Criterion, Seed, CASES},
            score,
        };
        let case = Case {
            memories: &[Seed {
                text: "Atlas deploys with ember.",
                group: "deploy",
                needed_on: &[0],
                related_on: &[],
                entities: &[],
            }],
            followups: &[],
            rules: &[],
            changes: &[],
            criteria: &[
                Criterion {
                    id: "strict_delivery",
                    weight: 50,
                    gate: true,
                    description: "StrictDelivery",
                    label: "StrictDelivery",
                    check: Check::StrictDelivery,
                },
                Criterion {
                    id: "needed_injected",
                    weight: 50,
                    gate: true,
                    description: "NeededInjected",
                    label: "NeededInjected",
                    check: Check::NeededInjected { exchange: None },
                },
            ],
            ..CASES[29]
        };
        snapshot.prompts = vec![score::PromptRecord {
            turn_id: "t".into(),
            step: 0,
            system_prompt: "Helpful.\n\n# Memory — bank: e2e-control\nAutomatic memory.".into(),
        }];
        assert_snapshot_missing_gates(&case, snapshot, unavailable);
    }

    fn assert_snapshot_missing_gates(
        case: &crate::scenarios::memory::cases::Case,
        snapshot: crate::scenarios::memory::score::Snapshot,
        unavailable: bool,
    ) {
        use crate::report::{gate_completion, CompletionState, CriterionReport};
        use crate::scenarios::memory::score;
        let evidence = score::evidence(case, "control", &transcript(), snapshot);
        let evaluation = score::evaluate(case, &evidence);
        for id in ["strict_delivery", "needed_injected"] {
            assert_eq!(
                evaluation
                    .awards
                    .iter()
                    .find(|award| award.id == id)
                    .unwrap()
                    .awarded,
                Some(0),
                "{id}"
            );
        }
        let reports: Vec<_> = score::criteria(case)
            .iter()
            .map(|criterion| {
                let award = evaluation
                    .awards
                    .iter()
                    .find(|a| a.id == criterion.id)
                    .unwrap();
                CriterionReport {
                    id: criterion.id.into(),
                    description: None,
                    label: None,
                    possible: criterion.weight,
                    awarded: award.awarded,
                    reason: award.reason.clone(),
                    gate: criterion.gate,
                    gate_minimum: criterion.gate_minimum,
                }
            })
            .collect();
        assert_eq!(
            evaluation
                .completion
                .unwrap_or_else(|| gate_completion(&reports)),
            CompletionState::TaskIncomplete
        );
        assert_eq!(evaluation.infrastructure_error.is_some(), unavailable);
    }

    #[test]
    fn identical_user_survivor_is_not_location_provenance() {
        let captured = data(&event(A, 41, true));
        assert!(captured.available());
        // Same actual final payload in two worlds: authentic notice survived,
        // or notice removed and ordinary identical user text survived alone.
        let legitimate_payload =
            json!({"messages":[notice()["custom"]["data"]["message"].clone()]});
        let ordinary_survivor_payload =
            json!({"messages":[{"role":"user","content":[{"type":"text","text":memory_body()}]}]});
        assert_eq!(legitimate_payload, ordinary_survivor_payload);
        assert_eq!(captured.delivery(&transcript()), None);
        assert_named_missing_gates(captured, true);
    }

    #[test]
    fn unrelated_invalid_subjects_do_not_poison_own_complete_pair() {
        let mut captured = data(&event(A, 41, false));
        for phase in ["request", "error", "cancelled"] {
            let mut foreign = serde_json::to_value(event(B, 501, false)).unwrap();
            foreign["session_id"] = json!("unrelated");
            foreign["complete"] = json!(false);
            foreign["phase"] = json!(phase);
            captured.record(
                if phase == "request" {
                    REQUEST
                } else {
                    RESPONSE
                },
                foreign,
            );
        }
        captured.record(
            UPDATE,
            json!({"input":{"session_id":"unrelated"},"output":{"updated":false}}),
        );
        for text in ["first", "conflicting"] {
            captured.record(
                NOTICE,
                json!({"session_id":"unrelated","entry_id":"foreign","text":text}),
            );
        }
        assert!(captured.available());
        assert_eq!(captured.delivery(&transcript()), Some(false));
        let mut collision = serde_json::to_value(event(A, 41, false)).unwrap();
        collision["session_id"] = json!("unrelated");
        captured.record(REQUEST, collision);
        assert_eq!(captured.delivery(&transcript()), None);
        let mut reverse = Captured {
            session: "e2e_control".into(),
            ..Captured::default()
        };
        let mut foreign = serde_json::to_value(event(A, 41, false)).unwrap();
        foreign["session_id"] = json!("unrelated");
        reverse.record(REQUEST, foreign);
        reverse.record(REQUEST, serde_json::to_value(event(A, 41, false)).unwrap());
        assert!(reverse
            .rejected_generations
            .contains(&(INSTANCE.into(), A.into())));
    }

    #[test]
    fn malformed_foreign_traffic_in_both_orders_preserves_own_pair() {
        let own = event(A, 41, false);
        let foreign = [
            (REQUEST, json!({"session_id":"foreign"})),
            (RESPONSE, json!({"session_id":"foreign","phase":"error"})),
            (
                UPDATE,
                json!({"input":{"session_id":"foreign"},"output":{"updated":false}}),
            ),
            (
                NOTICE,
                json!({"session_id":"foreign","entry_id":"x","text":"first"}),
            ),
            (
                NOTICE,
                json!({"session_id":"foreign","entry_id":"x","text":"conflict"}),
            ),
        ];
        for before in [true, false] {
            let mut captured = Captured {
                ready: true,
                session: "e2e_control".into(),
                bank: "e2e-control".into(),
                ..Captured::default()
            };
            if before {
                for (source, payload) in &foreign {
                    captured.record(source, payload.clone());
                }
            }
            captured.record(HOOK, hook());
            captured.record(NOTICE, notice());
            captured.record(REQUEST, serde_json::to_value(&own).unwrap());
            captured.record(RESPONSE, terminal(&own));
            captured.record(UPDATE, update(A));
            if !before {
                for (source, payload) in &foreign {
                    captured.record(source, payload.clone());
                }
            }
            assert!(captured.available());
            assert_eq!(captured.delivery(&transcript()), Some(false));
        }
    }

    #[test]
    fn producer_identity_scopes_collisions_and_own_conflicts() {
        let own = event(A, 41, false);
        let mut foreign = own.clone();
        foreign.session_id = "foreign".into();
        foreign.producer_instance = B.into();
        for before in [true, false] {
            let mut captured = if before {
                Captured {
                    ready: true,
                    session: "e2e_control".into(),
                    bank: "e2e-control".into(),
                    ..Captured::default()
                }
            } else {
                data(&own)
            };
            captured.record(REQUEST, serde_json::to_value(&foreign).unwrap());
            if before {
                captured.record(HOOK, hook());
                captured.record(NOTICE, notice());
                captured.record(REQUEST, serde_json::to_value(&own).unwrap());
                captured.record(RESPONSE, terminal(&own));
                captured.record(UPDATE, update(A));
            }
            assert!(captured.available());
            let mut conflict = own.clone();
            conflict.complete = false;
            captured.record(RESPONSE, serde_json::to_value(conflict).unwrap());
            assert!(!captured.available());
            let next = event(B, 45, false);
            captured.record(REQUEST, serde_json::to_value(&next).unwrap());
            captured.record(RESPONSE, terminal(&next));
            captured.record(UPDATE, update(B));
            assert!(
                captured.available(),
                "own conflict must not poison future generation"
            );
        }
    }

    #[test]
    fn failed_own_update_rejects_only_its_generation() {
        let mut captured = data(&event(A, 41, false));
        let mut failed = update(A);
        failed["output"]["updated"] = json!(false);
        captured.record(UPDATE, failed);
        assert!(!captured.available());
        assert_eq!(captured.delivery(&transcript()), None);
        let next = event(B, 45, false);
        captured.record(REQUEST, serde_json::to_value(&next).unwrap());
        captured.record(RESPONSE, terminal(&next));
        captured.record(UPDATE, update(B));
        assert!(captured.available());
        assert_eq!(captured.delivery(&transcript()), Some(false));
    }

    #[test]
    fn foreign_cache_and_private_hash_budget_are_bounded() {
        let mut captured = data(&event(A, 41, false));
        for sequence in 1000..(1000 + LIMIT as u64) {
            captured.record(REQUEST,json!({"session_id":"foreign","producer_instance":INSTANCE,"generation_id":B,"sequence":sequence}));
        }
        assert!(captured.foreign_sequences.len() + captured.foreign_generations.len() <= LIMIT);
        assert!(captured.available());
        let own = event(B, 99999, false);
        captured.record(REQUEST, serde_json::to_value(own).unwrap());
        assert!(captured.events.len() <= 3);
        let mut hashes = data(&event(A, 41, false));
        let mut oversized = event(B, 45, false);
        oversized.lines = vec![oversized.lines[0].clone(); HASH_LIMIT + 1];
        hashes.record(REQUEST, serde_json::to_value(oversized).unwrap());
        assert!(
            hashes
                .events
                .values()
                .map(|event| event.lines.len())
                .sum::<usize>()
                <= HASH_LIMIT
        );
        assert!(
            hashes.available(),
            "budget rejection must preserve retained own proof"
        );
        let mut generations = data(&event(A, 41, false));
        for index in 0..GENERATION_LIMIT + 2 {
            let mut next = event(
                &uuid::Uuid::new_v4().to_string(),
                1000 + 2 * index as u64,
                false,
            );
            next.dispatch_index = index as u64;
            generations.record(REQUEST, serde_json::to_value(next).unwrap());
        }
        assert_eq!(
            generations
                .events
                .values()
                .map(|event| &event.generation_id)
                .collect::<BTreeSet<_>>()
                .len(),
            GENERATION_LIMIT
        );
        assert!(generations.available());
    }

    #[test]
    fn ten_thousand_foreign_and_fake_own_rejections_preserve_exact_own_pairs_both_orders() {
        for before in [true, false] {
            let own = event(A, 41, true);
            let mut captured = if before {
                Captured {
                    ready: true,
                    session: "e2e_control".into(),
                    bank: "e2e-control".into(),
                    ..Captured::default()
                }
            } else {
                data(&own)
            };
            for index in 0..10_000_u64 {
                let foreign = json!({
                    "session_id":"foreign", "producer_instance":INSTANCE,
                    "generation_id":format!("foreign-{index}"), "sequence":1000 + index,
                    "complete":false,
                });
                captured.record_callback(REQUEST, foreign, true, true);
                // A claimed own session is not an authenticated subject. Neither
                // a bad caller nor an inactive activation may populate tombstones.
                let fake = json!({
                    "session_id":"e2e_control", "producer_instance":INSTANCE,
                    "generation_id":format!("fake-{index}"), "sequence":20000 + index,
                    "complete":false,
                });
                captured.record_callback(REQUEST, fake.clone(), false, false);
                captured.record_callback(RESPONSE, fake, false, false);
                captured.record_callback(
                    UPDATE,
                    json!({
                        "input":{"session_id":"e2e_control", "entry_id":format!("fake-{index}")},
                        "output":{"updated":false},
                    }),
                    false,
                    false,
                );
            }
            if before {
                captured.record(HOOK, hook());
                captured.record(NOTICE, notice());
                captured.record(REQUEST, serde_json::to_value(&own).unwrap());
                captured.record(RESPONSE, terminal(&own));
                captured.record(UPDATE, update(A));
            }
            assert!(captured.available(), "held own pair; before={before}");
            assert!(!captured.invalid);
            assert!(!captured.rejections_sealed);
            assert!(captured.rejected_generations.is_empty());
            assert!(captured.rejected_updates.is_empty());
            assert!(captured.foreign_sequences.len() + captured.foreign_generations.len() <= LIMIT);
            // Content-only v1 evidence still cannot certify the required point.
            assert_eq!(captured.delivery(&transcript()), None);
            let mut fresh = event(B, 40001, false);
            fresh.turn_id = "next".into();
            fresh.assistant_entry_id = "e_next_0_assistant".into();
            let mut outbound = update(B);
            outbound["input"]["entry_id"] = json!(fresh.assistant_entry_id);
            outbound["input"]["origin"]["turn_id"] = json!("next");
            captured.record(REQUEST, serde_json::to_value(&fresh).unwrap());
            captured.record(RESPONSE, terminal(&fresh));
            captured.record(UPDATE, outbound);
            let mut fresh_only = captured.clone();
            fresh_only
                .events
                .retain(|_, event| event.generation_id == B);
            assert!(
                fresh_only.available(),
                "fresh exact own pair; before={before}"
            );
            let mut held_only = captured.clone();
            held_only.events.retain(|_, event| event.generation_id == A);
            assert!(
                held_only.available(),
                "held exact own pair; before={before}"
            );
            // A real conflict from the authenticated owner still rejects B,
            // not the independent retained A pair, even after the noise cache fills.
            fresh.complete = false;
            captured.record_callback(REQUEST, serde_json::to_value(fresh).unwrap(), true, true);
            captured.events.retain(|_, event| event.generation_id == B);
            assert!(!captured.available());
        }
    }

    #[test]
    fn ten_thousand_owned_rejections_seal_only_bound_epoch_without_clean_replay() {
        let own = event(A, 41, true);
        let mut captured = data(&own);
        for index in 0..10_000_u64 {
            captured.record_callback(
                REQUEST,
                json!({
                    "session_id":"e2e_control", "producer_instance":INSTANCE,
                    "generation_id":format!("00000000-0000-4000-8000-{index:012x}"),
                    "sequence":1000 + index, "complete":false,
                }),
                true,
                true,
            );
        }
        assert_eq!(captured.rejected_generations.len(), LIMIT);
        assert!(captured.rejections_sealed);
        assert!(!captured.invalid);
        assert!(
            captured.available(),
            "unrelated retained proof is not globally poisoned"
        );
        assert_eq!(captured.delivery(&transcript()), None);
        captured.record(REQUEST, serde_json::to_value(&own).unwrap());
        captured.record(RESPONSE, terminal(&own));
        captured.record(UPDATE, update(A));
        assert!(
            captured.available(),
            "exact held pair redelivery after sealing"
        );
        // Both a retained tombstone and an overflow identity remain rejected;
        // there is no eviction or payload-sequence reset of this owner epoch.
        for index in [0_u64, 9999] {
            let id = format!("00000000-0000-4000-8000-{index:012x}");
            let replay = event(&id, 30001 + index * 2, false);
            let mut probe = captured.clone();
            probe.record(REQUEST, serde_json::to_value(&replay).unwrap());
            probe.record(RESPONSE, terminal(&replay));
            probe.record(UPDATE, update(&id));
            probe.events.retain(|_, event| event.generation_id != A);
            assert!(!probe.available(), "owned rejection replay {index}");
            assert_eq!(probe.rejected_generations.len(), LIMIT);
        }
        // A post-overflow conflict must revoke the retained exact pair too.
        let mut conflict = own;
        conflict.complete = false;
        captured.record(REQUEST, serde_json::to_value(conflict).unwrap());
        assert!(!captured.available());
        captured.record(REQUEST, serde_json::to_value(event(A, 41, true)).unwrap());
        captured.record(RESPONSE, terminal(&event(A, 41, true)));
        captured.record(UPDATE, update(A));
        assert!(
            !captured.available(),
            "removed owned conflict cannot replay clean"
        );
        // This is a separate trusted bind's empty accounting state, not an
        // event-declared producer UUID authorizing recovery in the old capture.
        let rebound = data(&event(B, 50001, true));
        assert!(rebound.available());
        assert!(!rebound.rejections_sealed);
        assert_eq!(rebound.delivery(&transcript()), None);
    }

    #[test]
    fn update_and_notice_rejection_bounds_preserve_independent_held_pair() {
        for source in [UPDATE, NOTICE] {
            let mut captured = data(&event(A, 41, true));
            for index in 0..10_000_u64 {
                if source == UPDATE {
                    captured.record_callback(
                        UPDATE,
                        json!({
                            "input":{"session_id":"e2e_control", "entry_id":format!("bad-{index}"),
                                "origin":{"generation_id":B}}, "output":{"updated":false},
                        }),
                        true,
                        true,
                    );
                } else {
                    // Test rejection accounting without filling the independent
                    // notice payload budget; failed authenticated callbacks use it.
                    captured.record_callback(
                        NOTICE,
                        json!({
                            "session_id":"e2e_control", "entry_id":format!("bad-{index}"),
                        }),
                        true,
                        false,
                    );
                }
            }
            assert!(captured.rejections_sealed);
            assert!(!captured.invalid);
            assert!(captured.available());
            assert_eq!(
                captured.rejected_updates.len() + captured.rejected_notices.len(),
                LIMIT
            );
            assert_eq!(captured.delivery(&transcript()), None);
            let mut conflict = update(A);
            conflict["output"]["updated"] = json!(false);
            captured.record_callback(UPDATE, conflict, true, true);
            assert!(
                !captured.available(),
                "current authenticated update conflict"
            );
            captured.record_callback(UPDATE, update(A), true, true);
            assert!(!captured.available(), "sealed update cannot replay clean");
        }
    }

    fn rejection_flood(captured: &mut Captured) {
        for index in 0..8193_u64 {
            captured.record_callback(
                REQUEST,
                json!({
                    "session_id":"e2e_control", "producer_instance":INSTANCE,
                    "generation_id":format!("00000000-0000-4000-8000-{index:012x}"),
                    "sequence":1000 + index, "complete":false,
                }),
                true,
                true,
            );
        }
        assert!(captured.rejections_sealed);
        assert_eq!(captured.rejected_generations.len(), LIMIT);
        assert!(captured.available());
    }

    fn update_id_cases() -> Vec<(&'static str, Option<Value>)> {
        vec![
            ("missing", None),
            ("null", Some(Value::Null)),
            ("number", Some(json!(7))),
            ("object", Some(json!({"id": A}))),
            ("empty", Some(json!(""))),
            ("whitespace", Some(json!(" \t\n\u{2003}"))),
            ("valid", Some(json!(A))),
        ]
    }

    fn with_update_id(mut payload: Value, generation: Option<Value>) -> Value {
        let origin = payload["input"]["origin"].as_object_mut().unwrap();
        if let Some(generation) = generation {
            origin.insert("generation_id".into(), generation);
        } else {
            origin.remove("generation_id");
        }
        payload
    }

    #[test]
    fn own_update_id_exact_probe_before_and_after_8193_rejections() {
        for flooded in [false, true] {
            let mut baseline = data(&event(A, 41, false));
            if flooded {
                rejection_flood(&mut baseline);
            }
            assert_eq!(baseline.delivery(&transcript()), Some(false));
            for (name, generation) in update_id_cases() {
                // Successful output, explicit failure, absent result field,
                // absent response, and a failed authenticated SDK invocation.
                for outcome in 0..5 {
                    let mut captured = baseline.clone();
                    let mut payload = with_update_id(update(A), generation.clone());
                    match outcome {
                        0 | 4 => {}
                        1 => payload["output"]["updated"] = json!(false),
                        2 => {
                            payload["output"].as_object_mut().unwrap().remove("updated");
                        }
                        3 => {
                            payload.as_object_mut().unwrap().remove("output");
                        }
                        _ => unreachable!(),
                    }
                    captured.record_callback(UPDATE, payload, true, outcome != 4);
                    // Updates without a usable generation are streamed partials:
                    // ignored, so the stamped proof stands whatever their outcome.
                    let accepted = name != "valid" || outcome == 0;
                    assert_eq!(
                        captured.available(),
                        accepted,
                        "id={name}, outcome={outcome}, flooded={flooded}"
                    );
                    assert_eq!(
                        captured.delivery(&transcript()),
                        accepted.then_some(false),
                        "delivery id={name}, outcome={outcome}, flooded={flooded}"
                    );
                    assert!(
                        !captured.invalid,
                        "entry-scoped rejection, not global poison"
                    );
                    assert!(
                        captured.rejected_generations.len()
                            + captured.rejected_entries.len()
                            + captured.rejected_updates.len()
                            + captured.rejected_notices.len()
                            <= LIMIT
                    );
                    if !accepted {
                        captured.record_callback(UPDATE, update(A), true, true);
                        assert!(!captured.available(), "old proof cannot replay clean");
                        assert_eq!(captured.delivery(&transcript()), None);
                    }
                }
            }
        }
    }

    #[test]
    fn malformed_update_ids_cannot_poison_foreign_or_unauthenticated_own_subjects() {
        for flooded in [false, true] {
            let mut baseline = data(&event(A, 41, false));
            if flooded {
                rejection_flood(&mut baseline);
            }
            for (name, generation) in update_id_cases() {
                for (subject, authenticated_source, authentic) in [
                    ("foreign", true, true),
                    ("foreign", true, false),
                    ("e2e_control", false, false),
                ] {
                    for updated in [false, true] {
                        let mut captured = baseline.clone();
                        let mut payload = with_update_id(update(A), generation.clone());
                        payload["input"]["session_id"] = json!(subject);
                        payload["output"]["updated"] = json!(updated);
                        captured.record_callback(UPDATE, payload, authenticated_source, authentic);
                        assert!(
                            captured.available(),
                            "foreign/forged id={name}, flooded={flooded}, updated={updated}"
                        );
                        assert_eq!(captured.delivery(&transcript()), Some(false));
                        assert!(!captured.invalid);
                    }
                }
            }
        }
    }

    #[test]
    fn malformed_own_update_keeps_independent_proof_and_trusted_recovery() {
        for flooded in [false, true] {
            let mut baseline = data(&event(A, 41, false));
            let mut fresh = event(B, 45, false);
            fresh.version = 2;
            fresh.origins_complete = true;
            fresh.turn_id = "next".into();
            fresh.assistant_entry_id = "e_next_0_assistant".into();
            let mut outbound = update(B);
            outbound["input"]["entry_id"] = json!(fresh.assistant_entry_id);
            outbound["input"]["origin"]["turn_id"] = json!("next");
            baseline.record_callback(REQUEST, serde_json::to_value(&fresh).unwrap(), true, true);
            baseline.record_callback(RESPONSE, terminal(&fresh), true, true);
            baseline.record_callback(UPDATE, outbound.clone(), true, true);
            if flooded {
                rejection_flood(&mut baseline);
            }
            for (name, generation) in update_id_cases() {
                // A failed partial without a generation is ignored; a failed
                // stamped update rejects its own generation only.
                let affected = (name != "valid").then_some(false);
                let mut captured = baseline.clone();
                let mut failed = with_update_id(update(A), generation);
                failed["output"]["updated"] = json!(false);
                captured.record_callback(UPDATE, failed, true, true);
                assert!(captured.available(), "independent retained B; id={name}");
                assert_eq!(
                    captured.delivery(&transcript()),
                    affected,
                    "affected A; id={name}"
                );
                let mut fresh_transcript = transcript();
                fresh_transcript["messages"][1]["entry_id"] = json!(fresh.assistant_entry_id);
                fresh_transcript["messages"][1]["origin"]["turn_id"] = json!("next");
                assert_eq!(captured.delivery(&fresh_transcript), Some(true));
                captured.record_callback(UPDATE, update(A), true, true);
                assert_eq!(
                    captured.delivery(&transcript()),
                    affected,
                    "A remains rejected"
                );
                assert!(!captured.invalid);
                // A new legitimate generation can be admitted before sealing;
                // after sealing recovery needs a new trusted bind, not a reset.
                let mut recovery = data(&event(A, 41, false));
                if flooded {
                    rejection_flood(&mut recovery);
                }
                let mut failed = update(A);
                failed["output"]["updated"] = json!(false);
                recovery.record_callback(UPDATE, failed, true, true);
                assert!(!recovery.available());
                if flooded {
                    recovery = Captured {
                        ready: true,
                        session: "e2e_control".into(),
                        bank: "e2e-control".into(),
                        ..Captured::default()
                    };
                }
                recovery.record_callback(HOOK, hook(), true, true);
                recovery.record_callback(NOTICE, notice(), true, true);
                recovery.record_callback(
                    REQUEST,
                    serde_json::to_value(&fresh).unwrap(),
                    true,
                    true,
                );
                recovery.record_callback(RESPONSE, terminal(&fresh), true, true);
                recovery.record_callback(UPDATE, outbound.clone(), true, true);
                assert!(recovery.available());
                assert_eq!(recovery.delivery(&fresh_transcript), Some(true));
            }
        }
    }

    #[test]
    fn update_identity_is_nonempty_but_never_trimmed_or_uuid_normalized() {
        for id in [A.to_string(), format!(" {A} "), "opaque identity".into()] {
            let input = json!({"origin":{"generation_id":id}});
            assert_eq!(
                update_generation(&input),
                input["origin"]["generation_id"].as_str()
            );
        }
        // A padded UUID is not the same generation as the exact producer UUID.
        let mut captured = data(&event(A, 41, false));
        captured.updates.clear();
        captured.record_callback(UPDATE, update(&format!(" {A} ")), true, true);
        assert!(!captured.available());
        assert_eq!(captured.delivery(&transcript()), None);
        // A valid update without its own terminal still cannot prove delivery.
        for (name, generation) in update_id_cases() {
            let mut pending = Captured {
                ready: true,
                session: "e2e_control".into(),
                bank: "e2e-control".into(),
                ..Captured::default()
            };
            pending.record_callback(
                REQUEST,
                serde_json::to_value(event(A, 41, false)).unwrap(),
                true,
                true,
            );
            pending.record_callback(UPDATE, with_update_id(update(A), generation), true, true);
            assert!(!pending.available(), "no own response; id={name}");
            assert_eq!(pending.delivery(&transcript()), None);
        }
    }

    struct Callback {
        source: &'static str,
        payload: Value,
        caller: &'static str,
        activation: bool,
        done: oneshot::Sender<Value>,
    }

    /// Ordinary engine-shaped invokefunction -> SDK task-local context -> real
    /// registered callback -> real collector, without a model/provider/engine.
    async fn sdk_session() -> (
        E2eContext,
        mpsc::Sender<Callback>,
        Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (tx, mut rx) = mpsc::channel::<Callback>(16);
        let mode = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_mode = mode.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.send(Message::Text(json!({"type":"workerregistered","worker_id":"reader","features":["observe.v2"]}).to_string())).await.unwrap();
            let mut observers = BTreeMap::<String, (String, String)>::new();
            let mut pending = BTreeMap::<String, oneshot::Sender<Value>>::new();
            let mut frames = 0;
            loop {
                tokio::select! {
                    command = rx.recv() => {
                        let Some(command) = command else { break; };
                        let (observer,target) = observers.get(command.source).unwrap();
                        let id = uuid::Uuid::new_v4().to_string();
                        if server_mode.load(std::sync::atomic::Ordering::SeqCst) == 2 {
                            socket.send(Message::Text(json!({"type":"observerstatechanged","id":observer,"state":"suspended","fires":0}).to_string())).await.unwrap();
                        }
                        let activation = uuid::Uuid::new_v4().to_string();
                        let mut envelope = if matches!(command.source, UPDATE | HOOK | APPEND) { command.payload } else { json!({"output":command.payload}) };
                        envelope["observer_id"] = json!(observer);
                        envelope["activation_id"] = json!(activation);
                        envelope["invocation_id"] = json!(uuid::Uuid::new_v4().to_string());
                        envelope["started_at_ms"] = json!(0);
                        envelope["duration_ms"] = json!(0.0);
                        envelope["function_id"] = json!(command.source);
                        envelope["namespace"] = json!("default");
                        envelope["status"] = json!("success");
                        if envelope.get("caller").is_none() { envelope["caller"] = json!({"worker_id":command.caller}); }
                        // Decode the real public SDK envelope, not just the
                        // permissive Value used by the collector callback.
                        let observed: iii_sdk::Observed<Value> = serde_json::from_value(envelope.clone()).unwrap();
                        assert_eq!(observed.observer_id, *observer);
                        assert_eq!(observed.activation_id, activation);
                        let mut message = json!({"type":"invokefunction","invocation_id":id,"function_id":target,"data":envelope,"caller":{"worker_name":"iii-engine","function_id":command.source},"observe_depth":1});
                        message["activation"] = json!({"observer_id":if command.activation {observer.as_str()} else {"forged"},"activation_id":activation});
                        pending.insert(id,command.done);
                        socket.send(Message::Text(message.to_string())).await.unwrap();
                    }
                    frame = socket.next() => {
                        let frame = match frame {
                            Some(Ok(Message::Text(frame))) => frame,
                            Some(Ok(Message::Ping(_))) => {
                                // The SDK sends an immediate keepalive after connecting.
                                // Control frames are not connection termination.
                                socket.flush().await.unwrap();
                                continue;
                            }
                            Some(Ok(Message::Pong(_))) => continue,
                            Some(Ok(Message::Close(_))) | None => break,
                            unexpected => panic!("unexpected synthetic frame {unexpected:?}"),
                        };
                        let message:Value = serde_json::from_str(&frame).unwrap();
                        assert!(frames < 2048, "synthetic protocol exceeded bounded frame budget");
                        frames += 1;
                        // Frame budget is per independent transport session.
                        let response = match message["type"].as_str().unwrap() {
                            "registerworker" => Some(json!({"type":"workerregistered","worker_id":"reader","features":["observe.v2"]})),
                            "registerfunction" => Some(json!({"type":"functionregistered","id":message["id"],"namespace":"default","revision":0,"observers":[]})),
                            "observe" => {
                                observers.insert(message["source"]["function_id"].as_str().unwrap().into(),(message["id"].as_str().unwrap().into(),message["target"]["function_id"].as_str().unwrap().into()));
                                Some(json!({"type":"observeresult","id":message["id"],"state":if server_mode.load(std::sync::atomic::Ordering::SeqCst) == 1 {"error"} else {"active"},"error":if server_mode.load(std::sync::atomic::Ordering::SeqCst) == 1 {json!({"code":"FORBIDDEN","message":"synthetic refusal"})} else {Value::Null}}))
                            }
                            "invokefunction" => {
                                let result = match message["function_id"].as_str().unwrap() {
                                    "engine::workers::register" => json!({"id":"reader","registered":true}),
                                    "engine::functions::list" => json!({"functions":SOURCES.iter().map(|source|json!({"function_id":source,"namespace":"default","worker_name":if matches!(*source, REQUEST | RESPONSE | RECEIPT) {"harness"} else if *source == HOOK {"memory"} else {"session-manager"}})).collect::<Vec<_>>()}),
                                    "engine::workers::list" => json!({"workers":[{"name":"harness","namespace":"default","id":"harness-owner","status":"connected"},{"name":"session-manager","namespace":"default","id":"session-owner","status":"connected"},{"name":"memory","namespace":"default","id":"memory-owner","status":"connected"}]}),
                                    unexpected => panic!("unexpected function {unexpected}"),
                                };
                                message["invocation_id"].as_str().map(|id| json!({"type":"invocationresult","invocation_id":id,"function_id":message["function_id"],"result":result}))
                            }
                            "invocationresult" => {
                                if let Some(done) = pending.remove(message["invocation_id"].as_str().unwrap()) { let _ = done.send(message["result"].clone()); }
                                None
                            }
                            _ => None,
                        };
                        if let Some(response) = response { socket.send(Message::Text(response.to_string())).await.unwrap(); }
                    }
                }
            }
        });
        let client = iii_sdk::register_worker(
            &url,
            iii_sdk::InitOptions {
                identity: iii_sdk::WorkerIdentityMode::Explicit,
                namespace: Some("default".into()),
                ..iii_sdk::InitOptions::default()
            },
        );
        while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
            tokio::task::yield_now().await;
        }
        (E2eContext::from_client(client), tx, mode, server)
    }

    /// The SDK runs each callback in its own task, so an update can be
    /// recorded before its response, and the Harness streams unstamped
    /// partials into the same entry before and after its stamped final
    /// update. Sent in reverse emission order with those partials, on the
    /// runner's multi-thread runtime, delivery is still proved and nothing is
    /// tombstoned.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streamed_partials_and_reverse_order_still_prove_delivery() {
        tokio::time::timeout(Duration::from_secs(15), async {
            let (context, tx, _, server) = sdk_session().await;
            let capture = Capture::bind(&context, "control").await.unwrap();
            let request = v2_event();
            let mut observed_hook = hook();
            observed_hook["source_observers"] = json!(["memory-observer"]);
            let mut partial = with_update_id(update(A), None);
            partial["input"]["content"] = json!([{"type":"text","text":"emb"}]);
            let mut answers = Vec::new();
            // Every frame leaves the engine before the first callback answers.
            for (source, payload, caller) in [
                (HOOK, observed_hook, "harness-owner"),
                (
                    RECEIPT,
                    test_receipt(&request, &hook(), "memory-observer"),
                    "harness-owner",
                ),
                (NOTICE, notice(), "session-owner"),
                (UPDATE, partial.clone(), "harness-owner"),
                (UPDATE, update(A), "harness-owner"),
                (UPDATE, partial, "harness-owner"),
                (RESPONSE, terminal(&request), "harness-owner"),
                (
                    REQUEST,
                    serde_json::to_value(&request).unwrap(),
                    "harness-owner",
                ),
            ] {
                let (done, answer) = oneshot::channel();
                tx.send(Callback {
                    source,
                    payload,
                    caller,
                    activation: true,
                    done,
                })
                .await
                .unwrap();
                answers.push(answer);
            }
            for answer in answers {
                assert_eq!(answer.await.unwrap()["accepted"], true);
            }
            let data = capture.snapshot();
            assert!(
                data.rejected_updates.is_empty()
                    && data.rejected_entries.is_empty()
                    && data.rejected_generations.is_empty()
            );
            assert_eq!(data.updates["e_t_0_assistant"].len(), 1);
            assert!(data.available());
            assert_eq!(data.delivery(&transcript()), Some(true));
            drop(capture);
            drop(tx);
            context.client().shutdown();
            server.abort();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn synthetic_sdk_transport_populates_real_collector_and_rejects_forgery() {
        tokio::time::timeout(Duration::from_secs(15), async {
            let (context, tx, mode, server) = sdk_session().await;
            let capture = Capture::bind(&context,"control").await.unwrap();
            async fn send(tx:&mpsc::Sender<Callback>,source:&'static str,payload:Value,caller:&'static str,activation:bool) -> Value {
                let (done,received) = oneshot::channel();
                tx.send(Callback {source,payload,caller,activation,done}).await.unwrap(); received.await.unwrap()
            }
            assert_eq!(send(&tx,HOOK,hook(),"harness-owner",true).await["accepted"],true);
            assert_eq!(send(&tx,NOTICE,notice(),"session-owner",true).await["accepted"],true);
            let request = event(A,101,true);
            assert_eq!(send(&tx,REQUEST,serde_json::to_value(&request).unwrap(),"harness-owner",true).await["accepted"],true);
            assert_eq!(send(&tx,RESPONSE,terminal(&request),"harness-owner",true).await["accepted"],true);
            assert_eq!(send(&tx,UPDATE,update(A),"harness-owner",true).await["accepted"],true);
            assert!(capture.snapshot().available());
            assert_eq!(capture.snapshot().delivery(&transcript()),None);
            // The same SDK callback and runtime population seam must accept v2,
            // without inventing private/sourceEpoch registry descriptor fields.
            let positive = Capture::bind(&context,"control").await.unwrap();
            let mut receipt = hook();
            receipt["source_observers"] = json!(["memory-observer"]);
            let v2 = v2_event();
            for (source, payload, caller) in [
                (HOOK,receipt,"harness-owner"), (RECEIPT,test_receipt(&v2,&hook(),"memory-observer"),"harness-owner"), (NOTICE,notice(),"session-owner"),
                (REQUEST,serde_json::to_value(&v2).unwrap(),"harness-owner"),
                (RESPONSE,terminal(&v2),"harness-owner"),
                (UPDATE,update(A),"harness-owner")
            ] { assert_eq!(send(&tx,source,payload,caller,true).await["accepted"],true); }
            let mut populated = crate::scenarios::memory::score::Snapshot::default();
            positive.populate_snapshot(&mut populated);
            assert!(!populated.generation_messages_unavailable);
            assert_eq!(populated.generations.as_ref().unwrap().delivery(&transcript()),Some(true));
            assert_named_positive_gates(populated);
            drop(positive);
            // Genuine native run_step bytes, not consumer-generated source oracles.
            // The transport supplies engine caller/collector activation and a
            // nonempty synthetic assistant completion (native mock returns []).
            const NATIVE: [(&str, &str, Option<bool>); 9] = [
                ("both", include_str!("test_data/origin_memory_both.json"), Some(true)),
                ("retained", include_str!("test_data/origin_memory_retained.json"), Some(true)),
                ("removed", include_str!("test_data/origin_memory_removed.json"), Some(false)),
                ("summary", include_str!("test_data/origin_memory_summary.json"), Some(true)),
                ("foreign", include_str!("test_data/origin_memory_summary_foreign.json"), None),
                ("reordered", include_str!("test_data/origin_memory_summary_reordered.json"), None),
                ("noncontiguous", include_str!("test_data/origin_memory_summary_noncontiguous.json"), None),
                ("late", include_str!("test_data/origin_memory_summary_late.json"), None),
                ("basecopy", include_str!("test_data/origin_memory_summary_basecopy.json"), None),
            ];
            for (name, wire, baseline) in NATIVE {
              for summary_mutation in 0..if name=="summary" {13} else {1} {
                let expected=if summary_mutation==0 {baseline} else {None};
                let (context, tx, _, server) = sdk_session().await;
                let fixture: Value = serde_json::from_str(wire).unwrap();
                let native = Capture::bind(&context,"native").await.unwrap();
                let request = fixture["calls"].as_array().unwrap().iter().find(|call| call["function_id"] == REQUEST).unwrap()["data"].clone();
                { let mut data = native.data.lock().unwrap(); data.session = request["session_id"].as_str().unwrap().into(); data.bank = "synthetic-atlas".into(); }
                let mut entries = Vec::new();
                let mut native_prompt = String::new();
                for call in fixture["calls"].as_array().unwrap() {
                    let source = call["function_id"].as_str().unwrap_or_default();
                    if source == "router::chat" { native_prompt=call["data"]["system_prompt"].as_str().unwrap().into(); }
                    if call["type"] == "loopback-hook-output" {
                        send(&tx,HOOK,json!({"input":call["input"],"output":call["output"]}),"harness-owner",true).await;
                    } else if source == RECEIPT {
                        send(&tx,RECEIPT,call["data"].clone(),"harness-owner",true).await;
                    } else if source == APPEND && call.pointer("/data/custom/custom_type")==Some(&json!("model_notice")) {
                        let notice = call["data"].clone();
                        if !entries.iter().any(|entry: &Value| entry["entry_id"] == notice["entry_id"]) { entries.push(notice.clone()); }
                        send(&tx,NOTICE,notice,"session-owner",true).await;
                    } else if source == REQUEST || source == RESPONSE {
                        let mut payload=call["data"].clone();
                        if source==REQUEST && payload["summary"].is_object() {
                            match summary_mutation {
                                1=>payload["summary"]["assembly_id"]=json!(""),
                                2=>payload["summary"]["span"]["byte_start"]=payload["summary"]["span"]["byte_end"].clone(),
                                3=>payload["summary"]["span"]["byte_end"]=json!(u64::MAX),
                                4=>payload["summary"]["span"]["system_sha256"]=json!("invalid"),
                                5=>payload["summary"]["span"]["summary_sha256"]=json!("invalid"),
                                6=>payload["summary"]["inputs"][0]["observer_id"]=json!("foreign"),
                                7=>payload["summary"]["inputs"][0]["message_sha256"]=json!("0".repeat(64)),
                                8=>payload["summary"]["lines"].as_array_mut().unwrap().reverse(),
                                9=>payload["summary"]["lines"][1]["line"]=json!(100),
                                10=>payload["summary"]["span"]["system_sha256"]=json!("0".repeat(64)),
                                11=>payload["summary"]["span"]["summary_sha256"]=json!("0".repeat(64)),
                                12=>payload["summary"]["inputs"][0]["entry_id"]=json!("foreign"),_=>{}
                            }
                        }
                        // Valid-looking forged digests are checked against an
                        // independently retained terminal certificate, not by
                        // pretending the collector has a raw-prompt endpoint.
                        if source==RESPONSE && matches!(summary_mutation,10|11) {
                            let original=fixture["calls"].as_array().unwrap().iter().find(|call| call["function_id"]==REQUEST && call["data"]["generation_id"]==payload["generation_id"]).unwrap();
                            payload["summary"]=original["data"]["summary"].clone();
                        }
                        send(&tx,if source == REQUEST {REQUEST} else {RESPONSE},payload,"harness-owner",true).await;
                    } else if source == UPDATE && call.pointer("/data/origin/generation_id").and_then(Value::as_str).is_some() {
                        let mut input = call["data"].clone();
                        input["content"] = json!([{"type":"text","text":"ember"}]);
                        // Native mock has no recall-count diagnostic. Supply it
                        // independently of the source certificate under test.
                        input["origin"]["memory_recalled"]=json!(1);
                        send(&tx,UPDATE,json!({"input":input,"output":{"updated":true}}),"harness-owner",true).await;
                        entries.retain(|entry| entry["entry_id"] != input["entry_id"]);
                        entries.push(json!({"entry_id":input["entry_id"],"origin":input["origin"],"message":{"role":"assistant","content":input["content"]}}));
                    }
                }
                let transcript = json!({"messages":entries});
                let mut snapshot = crate::scenarios::memory::score::Snapshot::default();
                native.populate_snapshot(&mut snapshot);
                snapshot.prompts.push(crate::scenarios::memory::score::PromptRecord {turn_id:request["turn_id"].as_str().unwrap().into(),step:0,system_prompt:native_prompt});
                assert_eq!(snapshot.generations.as_ref().unwrap().delivery_for(&transcript,Some(&[vec!["Atlas deploys with ember.".into()]])),expected,"{name}");
                assert_native_gates(snapshot,&transcript,expected);
                eprintln!("native SDK {name} summary_mutation={summary_mutation}: delivery={expected:?}, named gates checked");
                drop(native);
                drop(tx); context.client().shutdown(); server.abort();
              }
            }
            // Every receipt field mutation travels through the registered SDK
            // callback. Missing, foreign or forged proofs remain unavailable; a
            // late receipt or an extra hook still counts, as callbacks arrive
            // in no fixed order.
            {
            let fixture: Value = serde_json::from_str(include_str!("test_data/origin_memory_both.json")).unwrap();
            let calls = fixture["calls"].as_array().unwrap();
            let hook = calls.iter().find(|call| call["type"] == "loopback-hook-output").unwrap();
            let notice = calls.iter().find(|call| call.pointer("/data/custom/custom_type")==Some(&json!("model_notice"))).unwrap()["data"].clone();
            let request = calls.iter().find(|call|call["function_id"]==REQUEST).unwrap()["data"].clone();
            let response = calls.iter().find(|call|call["function_id"]==RESPONSE).unwrap()["data"].clone();
            let receipt = calls.iter().find(|call|call["function_id"]==RECEIPT).unwrap()["data"].clone();
            for mutation in 0..34 {
                let (context, tx, mode, server) = sdk_session().await;
                let negative = Capture::bind(&context,"native").await.unwrap();
                { let mut data=negative.data.lock().unwrap(); data.session=request["session_id"].as_str().unwrap().into(); data.bank="synthetic-atlas".into(); }
                let mut changed=receipt.clone();
                match mutation {
                    0=>changed["version"]=json!(2), 1=>changed["receipt_id"]=json!("invalid"),
                    2=>changed["producer_instance"]=json!(uuid::Uuid::new_v4().to_string()),
                    3=>changed["session_id"]=json!("foreign"),4=>changed["turn_id"]=json!("foreign"),
                    5=>changed["step"]=json!(1),6=>changed["observer_id"]=json!("foreign"),
                    7=>changed["appends"][0]["append_ordinal"]=json!(1),
                    8=>changed["output_sha256"]=json!("0".repeat(64)),
                    9=>changed["appends"][0]["message_sha256"]=json!("0".repeat(64)), _=>{}
                }
                send(&tx,HOOK,json!({"input":hook["input"],"output":hook["output"]}),"harness-owner",true).await;
                if mutation!=10 && mutation!=11 && mutation!=12 {
                    if mutation==17 { mode.store(2,std::sync::atomic::Ordering::SeqCst); }
                    send(&tx,RECEIPT,changed.clone(),match mutation {15=>"attacker",18=>"new-owner",_=>"harness-owner"},mutation!=16).await;
                    mode.store(0,std::sync::atomic::Ordering::SeqCst);
                }
                if mutation==19 { send(&tx,HOOK,json!({"input":hook["input"],"output":hook["output"]}),"harness-owner",true).await; }
                send(&tx,NOTICE,notice.clone(),"session-owner",true).await;
                if mutation==12 { send(&tx,RECEIPT,changed.clone(),"harness-owner",true).await; }
                if mutation==13 { changed["output_sha256"]=json!("0".repeat(64)); send(&tx,RECEIPT,changed.clone(),"harness-owner",true).await; }
                if mutation==14 { send(&tx,RECEIPT,changed.clone(),"harness-owner",true).await; }
                let mut altered=request.clone();
                match mutation {
                    20=>altered["sources"][0]["observer_id"]=json!("foreign"),
                    21=>altered["sources"][0]["entry_id"]=json!("foreign"),
                    22=>altered["sources"][0]["append_ordinal"]=json!(1),
                    23=>altered["sources"][0]["message_sha256"]=json!("0".repeat(64)),
                    24=>altered["sources"][0]["source"]=json!("user"),
                    25=>altered["sources"][0]["message_index"]=json!(2048),
                    26=>altered["sources"]=json!([]),
                    27=>altered["origins_complete"]=json!(false),
                    28=>altered["complete"]=json!(false),
                    29=>{altered.as_object_mut().unwrap().remove("summary");},
                    30=>altered["version"]=json!(3),
                    31=>altered["sources"][0]["message_sha256"]=Value::Null,
                    32=>altered["summary"]=json!({}),
                    33=>altered["turn_id"]=json!("foreign"), _=>{}
                }
                send(&tx,REQUEST,altered,"harness-owner",true).await;
                if mutation==11 { send(&tx,RECEIPT,changed,"harness-owner",true).await; }
                send(&tx,RESPONSE,response.clone(),"harness-owner",true).await;
                let input=json!({"session_id":request["session_id"],"entry_id":request["assistant_entry_id"],"content":[{"type":"text","text":"ember"}],"origin":{"generation_id":request["generation_id"],"turn_id":request["turn_id"],"memory_bank":"synthetic-atlas","memory_recalled":1}});
                send(&tx,UPDATE,json!({"input":input,"output":{"updated":true}}),"harness-owner",true).await;
                let transcript=json!({"messages":[notice,{"entry_id":input["entry_id"],"origin":input["origin"],"message":{"role":"assistant","content":input["content"]}}]});
                let mut snapshot=crate::scenarios::memory::score::Snapshot::default(); negative.populate_snapshot(&mut snapshot);
                let expected=if matches!(mutation, 11 | 12 | 14 | 19) {Some(true)} else if mutation==26 {Some(false)} else {None};
                assert_eq!(snapshot.generations.as_ref().unwrap().delivery(&transcript),expected,"receipt/source mutation {mutation}");
                snapshot.prompts.push(crate::scenarios::memory::score::PromptRecord {turn_id:request["turn_id"].as_str().unwrap().into(),step:0,system_prompt:String::new()});
                assert_native_gates(snapshot,&transcript,expected);
                eprintln!("receipt/source SDK mutation {mutation}: delivery={expected:?}, named gates checked");
                drop(negative);
                drop(tx); context.client().shutdown(); server.abort();
            }
            mode.store(0, std::sync::atomic::Ordering::SeqCst);
            }
            // Complete actual callback capture with an observed missing fact.
            let missing = Capture::bind(&context,"control").await.unwrap();
            for (source, payload, caller) in [
                (HOOK,hook(),"harness-owner"), (NOTICE,notice(),"session-owner"),
                (REQUEST,serde_json::to_value(event(B,105,false)).unwrap(),"harness-owner"),
                (RESPONSE,terminal(&event(B,105,false)),"harness-owner"),
                (UPDATE,update(B),"harness-owner")
            ] { assert_eq!(send(&tx,source,payload,caller,true).await["accepted"],true); }
            assert!(missing.snapshot().available());
            assert_eq!(missing.snapshot().delivery(&transcript()),Some(false));
            assert_named_missing_gates(missing.snapshot(), false);
            drop(missing);
            eprintln!("synthetic checkpoint: authentic collector available; notice provenance unavailable in v1");
            // Exercise the same snapshot population used by runtime, with real
            // callbacks missing one own half or the current update, not defaults.
            for omit in [REQUEST, RESPONSE, UPDATE] {
                let incomplete = Capture::bind(&context,"control").await.unwrap();
                for (source, payload, caller) in [
                    (HOOK,hook(),"harness-owner"), (NOTICE,notice(),"session-owner"),
                    (REQUEST,serde_json::to_value(&request).unwrap(),"harness-owner"),
                    (RESPONSE,terminal(&request),"harness-owner"),
                    (UPDATE,update(A),"harness-owner")
                ] {
                    if source != omit { assert_eq!(send(&tx,source,payload,caller,true).await["accepted"],true); }
                }
                let mut snapshot = crate::scenarios::memory::score::Snapshot::default();
                incomplete.populate_snapshot(&mut snapshot);
                assert!(snapshot.generation_messages_unavailable, "missing {omit}");
                assert!(snapshot.generations.is_some());
                assert_missing_snapshot_gates(snapshot, true);
                if omit == UPDATE {
                    // Reading a decision snapshot must not discard the pending pair.
                    assert_eq!(send(&tx,UPDATE,update(A),"harness-owner",true).await["accepted"],true);
                    let mut completed = crate::scenarios::memory::score::Snapshot::default();
                    incomplete.populate_snapshot(&mut completed);
                    assert!(!completed.generation_messages_unavailable);
                    assert_eq!(completed.generations.unwrap().delivery(&transcript()),None);
                }
                drop(incomplete);
            }
            // Authenticated foreign malformed/error traffic is isolated through
            // the actual SDK adapter, not just the in-memory record helper.
            let foreign_noise = Capture::bind(&context,"control").await.unwrap();
            for before in [true,false] {
                if before {
                    assert_eq!(send(&tx,REQUEST,json!({"session_id":"foreign"}),"harness-owner",true).await["accepted"],true);
                    assert_eq!(send(&tx,RESPONSE,json!({"session_id":"foreign","phase":"error"}),"harness-owner",true).await["accepted"],true);
                }
                for (source,payload) in [(REQUEST,serde_json::to_value(&request).unwrap()),(RESPONSE,terminal(&request)),(UPDATE,update(A))] {
                    assert_eq!(send(&tx,source,payload,"harness-owner",true).await["accepted"],true);
                }
                if !before {
                    assert_eq!(send(&tx,REQUEST,json!({"session_id":"foreign"}),"harness-owner",true).await["accepted"],true);
                    assert_eq!(send(&tx,UPDATE,json!({"input":{"session_id":"foreign"},"output":{"updated":false}}),"harness-owner",true).await["accepted"],true);
                }
                assert!(foreign_noise.snapshot().available());
            }
            drop(foreign_noise);
            // Failed authentication cannot attribute even a claimed own session.
            let isolated = Capture::bind(&context,"control").await.unwrap();
            for forged_first in [true, false] {
                if forged_first { assert_eq!(send(&tx,REQUEST,json!({"session_id":"e2e_control"}),"attacker",true).await["accepted"],false); }
                for (source,payload) in [(REQUEST,serde_json::to_value(&request).unwrap()),(RESPONSE,terminal(&request)),(UPDATE,update(A))] {
                    assert_eq!(send(&tx,source,payload,"harness-owner",true).await["accepted"],true);
                }
                if !forged_first { assert_eq!(send(&tx,REQUEST,json!({"session_id":"e2e_control"}),"harness-owner",false).await["accepted"],false); }
                assert!(isolated.snapshot().available());
            }
            let mut conflict = request.clone(); conflict.lines.clear();
            assert_eq!(send(&tx,REQUEST,serde_json::to_value(conflict).unwrap(),"harness-owner",true).await["accepted"],true);
            assert!(!isolated.snapshot().available());
            drop(isolated);
            // Public caller fields inside a payload cannot repair either forged
            // engine caller or activation. Rebind isolates each negative control.
            for (caller,activation) in [("attacker",true),("harness-owner",false)] {
                let negative = Capture::bind(&context,"control").await.unwrap();
                assert_eq!(send(&tx,REQUEST,serde_json::to_value(&request).unwrap(),caller,activation).await["accepted"],false);
                assert_eq!(negative.snapshot().delivery(&transcript()),None);
                drop(negative);
            }
            let forged = Capture::bind(&context,"control").await.unwrap();
            let mut forged_update = update(A);
            forged_update["caller"]["worker_id"] = json!("attacker");
            assert_eq!(send(&tx,UPDATE,forged_update,"harness-owner",true).await["accepted"],false);
            assert_eq!(forged.snapshot().delivery(&transcript()),None);
            drop(forged);
            mode.store(1, std::sync::atomic::Ordering::SeqCst);
            assert!(Capture::bind(&context,"control").await.is_err());
            mode.store(0, std::sync::atomic::Ordering::SeqCst);
            // A suspended observer authenticates nothing.
            let negative = Capture::bind(&context,"control").await.unwrap();
            mode.store(2, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(send(&tx,REQUEST,serde_json::to_value(&request).unwrap(),"harness-owner",true).await["accepted"],false);
            assert_eq!(negative.snapshot().delivery(&transcript()),None);
            mode.store(0, std::sync::atomic::Ordering::SeqCst);
            drop(negative);
            // A worker that took over after bind is not the bound owner.
            let rotated = Capture::bind(&context,"control").await.unwrap();
            assert_eq!(send(&tx,REQUEST,serde_json::to_value(&request).unwrap(),"new-owner",true).await["accepted"],false);
            drop(rotated);
            drop(capture); drop(tx); context.client().shutdown(); server.abort();
            eprintln!("synthetic checkpoint: forged caller/activation, refusal, inactive and foreign owner rejected");
        }).await.unwrap();
    }
}
