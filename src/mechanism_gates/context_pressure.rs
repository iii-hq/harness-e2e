//! E1: the context-manager engages once the ingested context outgrows the
//! usable window, compacts into a summary the next prompt carries, and only
//! reduces results in an emergency on a step that compacts.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{customs, messages, text, turn_step, Attempt, Broken, Prompt, Run};
use crate::wire::SessionMetricsResponse;

pub const ID: &str = crate::scenarios::context_pressure::ID;
/// How the context-manager renders a result its emergency pass reduced.
pub const EMERGENCY_REFERENCE: &str = "[function result reduced for context budget]";
/// The heading `assemble` renders the summary under in the system prompt.
const SUMMARY_HEADING: &str = "# Conversation summary";
/// The context-manager's summary template, in order.
pub(crate) const TEMPLATE_HEADINGS: [&str; 8] = [
    "## Goal",
    "## Constraints & Preferences",
    "## Progress",
    "## Key Decisions",
    "## Actions Taken",
    "## Next Steps",
    "## Critical Context",
    "## Relevant Files",
];

/// The compaction records in a transcript.
pub(crate) fn compactions(transcript: &Value) -> Vec<&Value> {
    customs(transcript, "compaction").collect()
}

/// The root session's last context snapshot.
fn root_context(metrics: Option<&SessionMetricsResponse>) -> Option<&Value> {
    metrics?
        .by_session
        .iter()
        .find(|session| session.depth == 0)?
        .context
        .as_ref()
}

/// Prompt tokens the provider billed for each root step, in order.
fn billed_prompts(transcript: &Value) -> Vec<u64> {
    messages(transcript)
        .iter()
        .filter(|entry| text(entry, "/message/role") == Some("assistant"))
        .filter_map(|entry| entry.pointer("/message/usage"))
        .map(|usage| {
            ["input", "cache_read", "cache_write"]
                .iter()
                .filter_map(|field| usage.get(field).and_then(Value::as_u64))
                .sum()
        })
        .collect()
}

/// The context the root would have needed uncompacted, over its usable
/// window: the last billed prompt plus every head a compaction summarized.
fn pressure(transcript: &Value, metrics: Option<&SessionMetricsResponse>) -> Option<f64> {
    let usable = root_context(metrics)?.get("usable")?.as_u64()?;
    let last = *billed_prompts(transcript).last()?;
    let summarized: u64 = compactions(transcript)
        .iter()
        .filter_map(|entry| {
            entry
                .pointer("/custom/data/summarized_head_tokens")?
                .as_u64()
        })
        .sum();
    (usable > 0).then(|| (last + summarized) as f64 / usable as f64)
}

/// The longitudinal measurements the context report carries.
pub fn measurements(transcript: &Value, metrics: &SessionMetricsResponse) -> Vec<Value> {
    let mut measured = vec![json!({
        "id": "compactions", "value": compactions(transcript).len(), "unit": "compactions",
    })];
    if let Some(peak) = billed_prompts(transcript).into_iter().max() {
        measured.push(json!({"id": "peak_prompt_tokens", "value": peak, "unit": "tokens"}));
    }
    if let Some(pressure) = pressure(transcript, Some(metrics)) {
        measured.push(json!({"id": "context_pressure", "value": pressure, "unit": "ratio"}));
    }
    measured
}

/// Whether every summary keeps the template's eight headings in order and
/// the charter's values; `None` when nothing was compacted.
pub fn summary_template(transcript: &Value, charter: &[&str]) -> Option<bool> {
    let summaries: Vec<&str> = compactions(transcript)
        .iter()
        .map(|entry| text(entry, "/custom/data/summary").unwrap_or_default())
        .collect();
    (!summaries.is_empty()).then(|| {
        summaries.iter().all(|summary| {
            let mut rest = *summary;
            TEMPLATE_HEADINGS
                .iter()
                .all(|heading| match rest.find(heading) {
                    Some(at) => {
                        rest = &rest[at + heading.len()..];
                        true
                    }
                    None => false,
                })
                && charter.iter().all(|value| summary.contains(value))
        })
    })
}

/// Joins each compaction with the prompt the observer saw on its step.
pub(super) fn capture(
    attempt: &Attempt<'_>,
    prompts: &[Prompt],
    probes: &mut BTreeMap<String, Value>,
) {
    let carried: Vec<Value> = compactions(attempt.transcript)
        .iter()
        .map(|entry| {
            let id = text(entry, "/entry_id").unwrap_or_default();
            let summary = text(entry, "/custom/data/summary").unwrap_or_default();
            let wanted = format!("{SUMMARY_HEADING}\n\n{summary}");
            let in_prompt = turn_step(id).is_some_and(|(turn, step)| {
                prompts.iter().any(|prompt| {
                    prompt.turn_id == turn
                        && prompt.step == step
                        && prompt.system_prompt.contains(&wanted)
                })
            });
            json!({"entry_id": id, "in_prompt": in_prompt})
        })
        .collect();
    let emergency: Vec<Value> = prompts
        .iter()
        .filter(|prompt| prompt.emergency_references > 0)
        .map(|prompt| {
            json!({"turn_id": prompt.turn_id, "step": prompt.step,
                   "references": prompt.emergency_references})
        })
        .collect();
    probes.insert("observed_prompts".into(), json!(prompts.len()));
    probes.insert("summary_in_prompt".into(), json!(carried));
    probes.insert("emergency".into(), json!(emergency));
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let compacted = compactions(run.transcript);
    let ids: Vec<&str> = messages(run.transcript)
        .iter()
        .filter_map(|entry| text(entry, "/entry_id"))
        .collect();
    let steps: Vec<(&str, u64)> = compacted
        .iter()
        .filter_map(|entry| turn_step(text(entry, "/entry_id")?))
        .collect();
    for entry in &compacted {
        let id = text(entry, "/entry_id").unwrap_or("an unnamed compaction");
        if text(entry, "/custom/data/summary").is_none_or(|summary| summary.trim().is_empty()) {
            broken.push((
                "context_manager_engaged",
                format!("{id} has an empty summary"),
            ));
        }
        let tail = text(entry, "/custom/data/tail_start_entry_id");
        if !tail.is_some_and(|tail| ids.contains(&tail)) {
            broken.push((
                "context_manager_engaged",
                format!("{id} starts its tail at {tail:?}, which no entry has"),
            ));
        }
    }
    for pair in steps.windows(2) {
        if pair[0].0 == pair[1].0 && pair[1].1 == pair[0].1 + 1 {
            broken.push((
                "context_manager_engaged",
                format!(
                    "compacted on consecutive steps {} and {} of {}",
                    pair[0].1, pair[1].1, pair[0].0
                ),
            ));
        }
    }
    if let Some(pressure) = pressure(run.transcript, run.metrics).filter(|p| *p >= 1.0) {
        let most = pressure.ceil() as usize + 1;
        if !(1..=most).contains(&compacted.len()) {
            broken.push((
                "context_manager_engaged",
                format!(
                    "{} compaction(s) at pressure {pressure:.2}; expected 1 to {most}",
                    compacted.len()
                ),
            ));
        }
        let flagged = root_context(run.metrics)
            .and_then(|context| context.get("compacted"))
            .and_then(Value::as_bool);
        if flagged != Some(true) {
            broken.push((
                "context_manager_engaged",
                format!(
                    "the root snapshot reports compacted={flagged:?} at pressure {pressure:.2}"
                ),
            ));
        }
    }
    if run.probe("observed_prompts").and_then(Value::as_u64) == Some(0) {
        broken.push((
            "summary_in_prompt",
            "the pre_generate observer saw no root prompt".into(),
        ));
    }
    for carried in run
        .probe("summary_in_prompt")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if carried["in_prompt"] != true {
            broken.push((
                "summary_in_prompt",
                format!(
                    "the prompt on {}'s step lacks `{SUMMARY_HEADING}` and its summary",
                    carried["entry_id"]
                ),
            ));
        }
    }
    for reduced in run
        .probe("emergency")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let at = (
            reduced["turn_id"].as_str().unwrap_or_default(),
            reduced["step"].as_u64().unwrap_or_default(),
        );
        if !steps.contains(&at) {
            broken.push((
                "emergency_only_with_compaction",
                format!(
                    "{} result(s) reduced in an emergency on step {} of {} without a compaction",
                    reduced["references"], at.1, at.0
                ),
            ));
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Observed, SessionMetricsPayload, SessionUsage, SessionUsageTotals};

    fn assistant(turn: &str, step: u64, prompt: u64) -> Value {
        json!({"entry_id": format!("e_{turn}_{step}_assistant"), "message": {
            "role": "assistant", "usage": {"input": prompt / 4, "cache_read": prompt - prompt / 4},
        }})
    }

    fn compaction(turn: &str, step: u64, tail: &str, summary: &str, head: u64) -> Value {
        json!({"entry_id": format!("e_{turn}_{step}_compaction"), "custom": {
            "custom_type": "compaction",
            "data": {"summary": summary, "tail_start_entry_id": tail, "summarized_head_tokens": head},
        }})
    }

    fn metrics(usable: u64, compacted: bool) -> SessionMetricsResponse {
        Observed::from_normalized(SessionMetricsPayload {
            root_session_id: "root".into(),
            complete: true,
            totals: SessionUsageTotals::default(),
            by_session: vec![SessionUsage {
                session_id: "root".into(),
                parent_session_id: None,
                depth: 0,
                turns: 1,
                function_calls: 0,
                function_call_errors: 0,
                validation_retries: None,
                transient_resumes: None,
                wake_resumes: None,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
                cost_usd: None,
                context: Some(json!({"usable": usable, "compacted": compacted})),
                by_model: Vec::new(),
            }],
            traces: None,
        })
    }

    fn gates_of(
        entries: Vec<Value>,
        metrics: &SessionMetricsResponse,
        probes: Value,
    ) -> Vec<String> {
        let transcript = json!({"messages": entries});
        let probes = serde_json::from_value(probes).unwrap();
        gates(&Run {
            transcript: &transcript,
            metrics: Some(metrics),
            probes: &probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    #[test]
    fn a_pressured_run_that_compacted_once_into_its_prompt_keeps_every_gate() {
        let entries = vec![
            assistant("t1", 1, 90_000),
            compaction("t1", 2, "e_t1_1_assistant", "## Goal\n- read", 70_000),
            assistant("t1", 2, 40_000),
        ];
        let probes = json!({
            "observed_prompts": 2,
            "summary_in_prompt": [{"entry_id": "e_t1_2_compaction", "in_prompt": true}],
            "emergency": [{"turn_id": "t1", "step": 2, "references": 1}],
        });

        assert_eq!(
            gates_of(entries, &metrics(100_000, true), probes),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_pressured_run_that_never_compacted_fails_the_engagement_gate() {
        let entries = vec![assistant("t1", 1, 60_000), assistant("t1", 2, 120_000)];

        let broken = gates_of(entries, &metrics(100_000, false), json!({}));

        assert_eq!(broken.len(), 2, "{broken:?}");
        assert!(broken[0].contains("0 compaction(s) at pressure 1.20; expected 1 to 3"));
        assert!(broken[1].contains("compacted=Some(false)"));
    }

    #[test]
    fn a_run_within_its_window_is_not_asked_to_compact() {
        let entries = vec![assistant("t1", 1, 40_000)];

        assert!(gates_of(entries, &metrics(100_000, false), json!({})).is_empty());
    }

    #[test]
    fn thrashing_empty_or_dangling_compactions_and_lost_summaries_are_flagged() {
        let entries = vec![
            assistant("t1", 1, 90_000),
            compaction("t1", 2, "e_t1_1_assistant", "## Goal", 10_000),
            compaction("t1", 3, "e_gone", " ", 10_000),
            assistant("t1", 3, 90_000),
        ];
        let probes = json!({
            "observed_prompts": 2,
            "summary_in_prompt": [{"entry_id": "e_t1_2_compaction", "in_prompt": false}],
            "emergency": [{"turn_id": "t1", "step": 1, "references": 2}],
        });

        let broken = gates_of(entries, &metrics(100_000, true), probes);

        for expected in [
            "e_t1_3_compaction has an empty summary",
            "starts its tail at Some(\"e_gone\")",
            "consecutive steps 2 and 3 of t1",
            "summary_in_prompt: the prompt on \"e_t1_2_compaction\"'s step",
            "emergency_only_with_compaction: 2 result(s) reduced in an emergency on step 1 of t1",
        ] {
            assert!(
                broken.iter().any(|b| b.contains(expected)),
                "{expected} not in {broken:?}"
            );
        }
    }

    #[test]
    fn a_dead_observer_is_flagged() {
        let broken = gates_of(
            Vec::new(),
            &metrics(100_000, false),
            json!({"observed_prompts": 0}),
        );

        assert_eq!(
            broken,
            ["summary_in_prompt: the pre_generate observer saw no root prompt"]
        );
    }

    #[test]
    fn capture_joins_each_compaction_with_the_prompt_of_its_step() {
        let transcript = json!({"messages": [compaction("t1", 2, "e", "## Goal\n- read", 1)]});
        let prompt = |step, system_prompt: &str| Prompt {
            turn_id: "t1".into(),
            step,
            system_prompt: system_prompt.into(),
            emergency_references: 0,
            replays_append: false,
        };
        let metrics = metrics(1, false);
        let attempt = Attempt {
            run_id: "r",
            session_id: "root",
            filesystem_root: None,
            transcript: &transcript,
            metrics: &metrics,
            terminal_status: None,
        };
        let mut probes = BTreeMap::new();

        capture(
            &attempt,
            &[
                prompt(1, "base"),
                prompt(2, "base\n\n# Conversation summary\n\n## Goal\n- read"),
            ],
            &mut probes,
        );

        assert_eq!(
            probes["summary_in_prompt"],
            json!([{"entry_id": "e_t1_2_compaction", "in_prompt": true}])
        );
        assert_eq!(probes["observed_prompts"], 2);
    }

    #[test]
    fn the_summary_template_needs_every_heading_in_order_and_the_charter() {
        let full = TEMPLATE_HEADINGS.join("\n- x\n") + "\nCTX-M-1";
        let swapped = full
            .replace("## Goal", "## Gx")
            .replace("## Relevant Files", "## Relevant Files\n## Goal");
        let of = |summary: &str| {
            summary_template(
                &json!({"messages": [compaction("t1", 2, "e", summary, 1)]}),
                &["CTX-M-1"],
            )
        };

        assert_eq!(of(&full), Some(true));
        assert_eq!(of(&swapped), Some(false));
        assert_eq!(of(&full.replace("CTX-M-1", "")), Some(false));
        assert_eq!(
            summary_template(&json!({"messages": []}), &["CTX-M-1"]),
            None
        );
    }

    #[test]
    fn measurements_count_compactions_and_the_pressure() {
        let transcript = json!({"messages": [
            assistant("t1", 1, 90_000),
            compaction("t1", 2, "e_t1_1_assistant", "s", 70_000),
            assistant("t1", 2, 40_000),
        ]});

        let measured = measurements(&transcript, &metrics(100_000, true));

        assert_eq!(
            measured,
            [
                json!({"id": "compactions", "value": 1, "unit": "compactions"}),
                json!({"id": "peak_prompt_tokens", "value": 90_000, "unit": "tokens"}),
                json!({"id": "context_pressure", "value": 1.1, "unit": "ratio"}),
            ]
        );
    }
}
