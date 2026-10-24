//! Reads what memory injected into a session, per exchange, from the
//! transcript `session::messages` returns.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MEMORY_OPEN: &str = "<memory bank=";
const MEMORY_CLOSE: &str = "</memory>";
const REMINDER_OPEN: &str = "Standing instructions —";

/// One recall block memory appended on a step, as the `model_notice` entry holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryBlock {
    /// The bank its opening tag names.
    pub bank: String,
    pub turn_id: String,
    pub step: u64,
    /// Index in `transcript["messages"]`.
    pub position: usize,
    /// Memory texts, without the leading "- " and the trailing " (YYYY-MM-DD)".
    pub lines: Vec<String>,
    /// Bytes of the rendered "- … (date)" lines, newlines excluded: memory's own budget unit.
    pub memory_bytes: usize,
    /// Bytes from "Standing instructions —" up to "</memory>", 0 when the block has none.
    pub reminder_bytes: usize,
}

/// One turn, read from its step-0 assistant entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    pub turn_id: String,
    /// Position of the turn's step-0 assistant entry.
    pub position: usize,
    /// `origin.memory_bank` of that entry: proof memory ran.
    pub memory_bank: Option<String>,
    /// `origin.req.system_sha` of that entry.
    pub system_sha: Option<String>,
    /// Text of the turn's last assistant entry that has any.
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    pub position: usize,
    /// Position of the entry the kept tail opens at, when that entry is in the transcript.
    pub tail_start: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observed {
    pub exchanges: Vec<Exchange>,
    pub blocks: Vec<MemoryBlock>,
    pub compactions: Vec<Compaction>,
}

pub fn observe(transcript: &Value) -> Observed {
    let entries = transcript
        .pointer("/messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut observed = Observed {
        exchanges: Vec::new(),
        blocks: Vec::new(),
        compactions: Vec::new(),
    };
    for (position, entry) in entries.iter().enumerate() {
        if let Some(block) = memory_block(position, entry) {
            observed.blocks.push(block);
        } else if let Some(compaction) = compaction(position, entry, entries) {
            observed.compactions.push(compaction);
        } else if entry.pointer("/message/role").and_then(Value::as_str) == Some("assistant") {
            note_assistant(&mut observed.exchanges, position, entry);
        }
    }
    observed
}

/// Blocks a model request at `position` still holds: earlier blocks, minus
/// those a compaction before `position` summarized away.
pub fn visible_at(observed: &Observed, position: usize) -> Vec<&MemoryBlock> {
    // A compaction whose tail is unknown summarized everything before itself.
    let floor = observed
        .compactions
        .iter()
        .rev()
        .find(|compaction| compaction.position < position)
        .map(|compaction| compaction.tail_start.unwrap_or(compaction.position));
    observed
        .blocks
        .iter()
        .filter(|block| block.position < position && floor.is_none_or(|f| block.position >= f))
        .collect()
}

fn text_at<'a>(entry: &'a Value, pointer: &str) -> Option<&'a str> {
    entry.pointer(pointer).and_then(Value::as_str)
}

fn memory_block(position: usize, entry: &Value) -> Option<MemoryBlock> {
    if text_at(entry, "/custom/custom_type") != Some("model_notice")
        || text_at(entry, "/custom/data/kind") != Some("hook")
    {
        return None;
    }
    let text = text_at(entry, "/custom/data/text")?;
    let (bank, _) = text
        .strip_prefix(MEMORY_OPEN)?
        .strip_prefix('"')?
        .split_once('"')?;
    // `e_{turn_id}_{step}_notice_{k}`; turn ids hold `_` themselves.
    let (head, _) = text_at(entry, "/entry_id")?
        .strip_prefix("e_")?
        .rsplit_once("_notice_")?;
    let (turn_id, step) = head.rsplit_once('_')?;

    let mut lines = Vec::new();
    let mut memory_bytes = 0;
    for line in text.lines().skip(1) {
        if line.starts_with(REMINDER_OPEN) || line.starts_with(MEMORY_CLOSE) {
            break;
        }
        if let Some(memory) = line.strip_prefix("- ") {
            memory_bytes += line.len();
            lines.push(strip_date(memory).to_string());
        }
    }
    let reminder_bytes = text
        .find(&format!("\n{REMINDER_OPEN}"))
        .zip(text.rfind(MEMORY_CLOSE))
        .and_then(|(start, end)| text.get(start + 1..end))
        .map_or(0, |reminder| reminder.trim_end().len());
    Some(MemoryBlock {
        bank: bank.to_string(),
        turn_id: turn_id.to_string(),
        step: step.parse().ok()?,
        position,
        lines,
        memory_bytes,
        reminder_bytes,
    })
}

/// "text (2026-10-23)" -> "text".
fn strip_date(line: &str) -> &str {
    let is_date = |date: &str| {
        date.len() == 10
            && date.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
    };
    line.strip_suffix(')')
        .and_then(|line| line.rsplit_once(" ("))
        .filter(|(_, date)| is_date(date))
        .map_or(line, |(text, _)| text)
}

fn compaction(position: usize, entry: &Value, entries: &[Value]) -> Option<Compaction> {
    if text_at(entry, "/custom/custom_type") != Some("compaction") {
        return None;
    }
    let tail_start = text_at(entry, "/custom/data/tail_start_entry_id").and_then(|tail| {
        entries
            .iter()
            .position(|other| text_at(other, "/entry_id") == Some(tail))
    });
    Some(Compaction {
        position,
        tail_start,
    })
}

fn note_assistant(exchanges: &mut Vec<Exchange>, position: usize, entry: &Value) {
    let Some(turn_id) = text_at(entry, "/origin/turn_id") else {
        return;
    };
    let text = entry
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| text_at(block, "/type") == Some("text"))
        .filter_map(|block| text_at(block, "/text"))
        .collect::<String>();
    let text = (!text.trim().is_empty()).then_some(text);
    match exchanges.iter_mut().find(|e| e.turn_id == turn_id) {
        Some(exchange) => {
            if let Some(text) = text {
                exchange.answer = text;
            }
        }
        None => exchanges.push(Exchange {
            turn_id: turn_id.to_string(),
            position,
            memory_bank: text_at(entry, "/origin/memory_bank").map(str::to_string),
            system_sha: text_at(entry, "/origin/req/system_sha").map(str::to_string),
            answer: text.unwrap_or_default(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn transcript(entries: Vec<Value>) -> Value {
        json!({ "messages": entries })
    }

    fn memory_text(extra: &str) -> String {
        format!(
            "<memory bank=\"e2e-x\">\nRelevant remembered memories (\"the user\" is the person you are talking to; recalled 2026-10-23 Friday; each dated when last updated; verify anything surprising):\n- Fact one. (2026-10-23)\n- Fact two. (2026-10-23)\n{extra}</memory>"
        )
    }

    fn notice(entry_id: &str, text: &str) -> Value {
        json!({"entry_id": entry_id, "custom": {"custom_type": "model_notice",
               "data": {"kind": "hook", "text": text}}})
    }

    fn assistant(turn: &str, step: u64, text: &str, origin_extra: Value) -> Value {
        let mut origin = json!({"turn_id": turn});
        if let (Some(base), Some(extra)) = (origin.as_object_mut(), origin_extra.as_object()) {
            base.extend(extra.clone());
        }
        json!({"entry_id": format!("e_{turn}_{step}_assistant"),
               "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
               "origin": origin})
    }

    fn filler(entry_id: &str) -> Value {
        json!({"entry_id": entry_id,
               "message": {"role": "user", "content": [{"type": "text", "text": "hi"}]}})
    }

    fn block_notice(entry_id: &str) -> Value {
        notice(entry_id, &memory_text(""))
    }

    fn compaction(entry_id: &str, tail: Value) -> Value {
        json!({"entry_id": entry_id, "custom": {"custom_type": "compaction",
               "data": {"summary": "s", "tail_start_entry_id": tail}}})
    }

    #[test]
    fn memory_block_is_found_behind_other_notices() {
        let observed = observe(&transcript(vec![
            notice("e_t_a_0_notice_0", "<harness>steer</harness>"),
            block_notice("e_t_a_0_notice_1"),
        ]));
        assert_eq!(observed.blocks.len(), 1);
        let block = &observed.blocks[0];
        assert_eq!(
            (block.turn_id.as_str(), block.step, block.position),
            ("t_a", 0, 1)
        );
        assert_eq!(block.bank, "e2e-x");
    }

    #[test]
    fn reminder_lines_are_not_memory_lines() {
        let reminder = "Standing instructions — apply every one to this reply:\n- a rule line\n";
        let observed = observe(&transcript(vec![notice(
            "e_t_a_0_notice_1",
            &memory_text(reminder),
        )]));
        let block = &observed.blocks[0];
        assert_eq!(block.lines, ["Fact one.", "Fact two."]);
        assert_eq!(
            block.memory_bytes,
            "- Fact one. (2026-10-23)".len() + "- Fact two. (2026-10-23)".len()
        );
        assert_eq!(block.reminder_bytes, reminder.trim_end().len());
    }

    #[test]
    fn a_block_without_a_reminder_has_none() {
        let observed = observe(&transcript(vec![block_notice("e_t_a_2_notice_0")]));
        assert_eq!(observed.blocks[0].reminder_bytes, 0);
        assert_eq!(observed.blocks[0].step, 2);
    }

    #[test]
    fn exchanges_follow_turns_and_read_the_step_zero_origin() {
        let sha = "ab".repeat(32);
        let observed = observe(&transcript(vec![
            filler("u1"),
            assistant("t_a", 0, "first", json!({})),
            filler("u2"),
            assistant(
                "t_b",
                0,
                "thinking",
                json!({"memory_bank": "e2e-x", "req": {"system_sha": sha}}),
            ),
            assistant("t_b", 1, "final answer", json!({"memory_bank": "e2e-x"})),
            assistant("t_b", 2, "  ", json!({})),
        ]));
        assert_eq!(observed.exchanges.len(), 2);
        assert_eq!(observed.exchanges[0].memory_bank, None);
        let second = &observed.exchanges[1];
        assert_eq!(second.turn_id, "t_b");
        assert_eq!(second.position, 3);
        assert_eq!(second.memory_bank.as_deref(), Some("e2e-x"));
        assert_eq!(second.system_sha.as_deref(), Some(sha.as_str()));
        assert_eq!(second.answer, "final answer");
    }

    #[test]
    fn rules_updates_are_not_memory_blocks() {
        let observed = observe(&transcript(vec![notice(
            "e_t_a_1_notice_0",
            "<memory update=\"rules\">\nThe memory rules changed.\n- a rule line\n</memory>",
        )]));
        assert!(observed.blocks.is_empty());
    }

    #[test]
    fn compaction_hides_blocks_before_its_tail() {
        let mut entries: Vec<Value> = (0..10).map(|i| filler(&format!("f{i}"))).collect();
        entries[1] = block_notice("e_t_a_0_notice_0");
        entries[5] = block_notice("e_t_b_0_notice_0");
        entries[9] = compaction("c1", json!("f4"));
        let observed = observe(&transcript(entries));
        assert_eq!(
            observed.compactions,
            [Compaction {
                position: 9,
                tail_start: Some(4)
            }]
        );
        let positions = |p| -> Vec<usize> {
            visible_at(&observed, p)
                .iter()
                .map(|b| b.position)
                .collect()
        };
        assert_eq!(positions(10), [5]);
        assert_eq!(positions(6), [1, 5]);
    }

    #[test]
    fn a_compaction_without_a_tail_hides_every_block_before_it() {
        let observed = observe(&transcript(vec![
            block_notice("e_t_a_0_notice_0"),
            compaction("c1", Value::Null),
            block_notice("e_t_b_0_notice_0"),
        ]));
        assert_eq!(observed.compactions[0].tail_start, None);
        let positions = |p| -> Vec<usize> {
            visible_at(&observed, p)
                .iter()
                .map(|b| b.position)
                .collect()
        };
        assert_eq!(positions(3), [2]);
        assert_eq!(positions(1), [0]);
    }
}
