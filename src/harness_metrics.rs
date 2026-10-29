//! Harness mechanisms whose numbers depend on the model or the environment,
//! so Trends charts them instead of gating on them, read from the root
//! session's transcript, its prompt parts and judge spans, and the
//! session-tree metrics the suite already captured. One gate rides along:
//! the repeated-failure breaker.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::audit::{AuditFlag, AuditFlagKind, AuditFlagSource, AuditSeverity};
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::{E2eRunReport, RunStatus};

/// What Trends charts per run, by the name its `<name>_mean` takes.
pub const TREND_KEYS: &[&str] = &[
    "trace_spans",
    "error_spans",
    "registry_notices",
    "hook_notices",
    "hook_guidance_tokens",
    "injected_prompt_tokens",
    "reconciled_calls",
    "reconciled_p95_ms",
    "functions_info_repeats",
    "duplicate_entry_events",
    "arguments_truncated_invalid_json",
    "arguments_truncated_incomplete_json",
    "arguments_truncated_max_output_tokens",
    "agent_trigger_no_target",
    "repeated_failure",
    "sweep_resolved_calls",
];

/// The keys read from somewhere other than the transcript.
const NOT_FROM_TRANSCRIPT: &[&str] = &[
    "trace_spans",
    "error_spans",
    "injected_prompt_tokens",
    "reconciled_p95_ms",
    "duplicate_entry_events",
];

/// The Harness's `REPEATED_FAILURE_LIMIT`: identical failures of one call a
/// turn runs before the next identical call is answered locally.
const REPEATED_FAILURE_LIMIT: u32 = 2;

/// Above it a run's reconciliations are flagged as slow.
const RECONCILED_P95_ALERT_MS: f64 = 2_500.0;

/// Calls the turn loop answers itself, on paths that never note a result
/// for the breaker: the `agent_trigger` wrapper with no target, the
/// `harness::spawn` guards, `harness::ask` and `submit_result`.
const CONTROL_CALLS: &[&str] = &[
    "agent_trigger",
    "harness::spawn",
    "harness::ask",
    "submit_result",
];

/// Answers the turn loop gives a dispatchable call without running its
/// target, results the sweep or a stop supplies, and the result of a call a
/// restart cut off: none reaches the breaker.
const NOT_DISPATCHED: &[&str] = &[
    "repeated_failure",
    "arguments_truncated",
    "agent_trigger_no_target",
    "policy_denied",
    "hook_denied",
    "pending_timeout",
    "cancelled",
    "interrupted",
];

const FUNCTIONS_INFO: &str = "engine::functions::info";

/// The engine's span for the Harness's call to the judge, the one step of
/// argument reconciliation that leaves the Harness: the schema repairs and
/// the diagnosis run in process.
pub const JUDGE_CALL_SPAN: &str = "call judge::evaluate";

/// A key is absent when its evidence is: no transcript, no traces, no
/// reconciliation that reached the judge, no prompt parts or no count of the
/// root session's `session::message-added` deliveries. A count over present
/// evidence is there even at zero, so a mean never skips a clean run.
pub fn measure(run: &E2eRunReport) -> BTreeMap<String, f64> {
    let mut measured = BTreeMap::new();
    for (key, value) in [
        (
            "duplicate_entry_events",
            run.duplicate_entry_events.map(|count| count as f64),
        ),
        (
            "injected_prompt_tokens",
            run.injected_prompt_tokens.map(|tokens| tokens as f64),
        ),
        (
            "reconciled_p95_ms",
            run.reconciliation_judge_ms.clone().and_then(p95),
        ),
    ] {
        if let Some(value) = value {
            measured.insert(key.to_string(), value);
        }
    }
    if let Some(traces) = run
        .metrics
        .as_ref()
        .and_then(|metrics| metrics.traces.as_ref())
    {
        for (key, field) in [
            ("trace_spans", "span_count"),
            ("error_spans", "error_span_count"),
        ] {
            if let Some(count) = traces.get(field).and_then(Value::as_u64) {
                measured.insert(key.to_string(), count as f64);
            }
        }
    }
    let Some(messages) = messages(run.transcript.as_ref()) else {
        return measured;
    };
    for key in TREND_KEYS {
        if !NOT_FROM_TRANSCRIPT.contains(key) {
            measured.insert(key.to_string(), 0.0);
        }
    }
    let mut count = |key: String| *measured.entry(key).or_insert(0.0) += 1.0;
    let mut info_payloads = Vec::new();
    let mut hook_bytes = 0;
    for entry in messages {
        match text(entry, "/custom/data/kind") {
            Some("registry-changed") => count("registry_notices".into()),
            Some("hook") => {
                count("hook_notices".into());
                hook_bytes += text(entry, "/custom/data/text").map_or(0, str::len);
            }
            _ => {}
        }
        for (function_id, payload) in content(entry).filter_map(planned) {
            if function_id == FUNCTIONS_INFO {
                if info_payloads.contains(&payload) {
                    count("functions_info_repeats".into());
                }
                info_payloads.push(payload);
            }
        }
        if is_reconciled(entry) {
            count("reconciled_calls".into());
        }
        match text(entry, "/message/details/error") {
            Some("arguments_truncated") => count(format!(
                "arguments_truncated_{}",
                text(entry, "/message/details/cause").unwrap_or("unknown")
            )),
            Some(code @ ("agent_trigger_no_target" | "repeated_failure")) => count(code.into()),
            // The run's share of the sweep's `resolved` counter.
            Some("pending_timeout") => count("sweep_resolved_calls".into()),
            _ => {}
        }
    }
    measured.insert("hook_guidance_tokens".into(), tokens(hook_bytes));
    measured
}

/// The breaker answers the third identical failure of a call in a turn
/// locally, so a passed run whose turn ran a call into the same error more
/// than twice ran past it. Like the Harness, a success clears a call's count
/// and a different error restarts it, and only a dispatched call's own
/// result counts.
/// shortcut: a call a hook held and an approver released runs through
/// `harness::function::resolve`, which notes nothing, yet its result looks
/// dispatched; only `approval_roundtrip` runs behind the approval gate and
/// the suite skips it, so read the hold from the turn record if another does.
pub fn repeated_failures(report: &E2eRunReport) -> Vec<HarnessViolation> {
    if report.status != RunStatus::Passed {
        return Vec::new();
    }
    let Some(messages) = messages(report.transcript.as_ref()) else {
        return Vec::new();
    };
    // The breaker keys a call on its target and the model's own arguments.
    let mut keys = BTreeMap::new();
    let mut failed = BTreeMap::<(&str, String), (Value, u32)>::new();
    let mut violations = Vec::new();
    for entry in messages {
        if text(entry, "/message/role") == Some("assistant") {
            for block in content(entry) {
                let id = block.get("id").and_then(Value::as_str);
                if let (Some(id), Some((target, payload))) = (id, planned(block)) {
                    if !CONTROL_CALLS.contains(&target) {
                        keys.insert(id, format!("{target} {payload}"));
                    }
                }
            }
            continue;
        }
        if text(entry, "/message/role") != Some("function_result")
            || text(entry, "/message/details/error")
                .is_some_and(|code| NOT_DISPATCHED.contains(&code))
        {
            continue;
        }
        let Some(key) = text(entry, "/message/function_call_id").and_then(|id| keys.get(id)) else {
            continue;
        };
        let slot = (
            text(entry, "/origin/turn_id").unwrap_or_default(),
            key.clone(),
        );
        if entry.pointer("/message/is_error").and_then(Value::as_bool) != Some(true) {
            failed.remove(&slot);
            continue;
        }
        let error = target_error(entry);
        let count = match failed.get(&slot) {
            Some((last, count)) if *last == error => count + 1,
            _ => 1,
        };
        if count == REPEATED_FAILURE_LIMIT + 1 {
            violations.push(HarnessViolation {
                invariant: HarnessInvariant::RepeatedFailureExecuted,
                detail: format!(
                    "turn {} ran {} into the same error {count} times",
                    slot.0, slot.1
                ),
            });
        }
        failed.insert(slot, (error, count));
    }
    violations
}

/// Advisory: the p95 of a run's reconciliation round trips to the judge past
/// 2.5 s.
pub fn slow_reconciled_flag(report: &E2eRunReport) -> Option<AuditFlag> {
    let p95 = p95(report.reconciliation_judge_ms.clone()?)?;
    (p95 > RECONCILED_P95_ALERT_MS).then(|| AuditFlag {
        kind: AuditFlagKind::SlowReconciledCalls,
        severity: AuditSeverity::Warning,
        source: AuditFlagSource::Deterministic,
        summary: format!("reconciliation took {p95} ms at the p95, past 2500 ms"),
        evidence: Vec::new(),
        confidence: None,
    })
}

/// The tokens of the prompt parts `pre_generate` hooks declare they inject,
/// from a `harness::system-prompt::get` preview.
pub fn injected_prompt_tokens(preview: &Value) -> Option<u64> {
    let parts = preview.get("parts")?.as_array()?;
    let bytes = parts
        .iter()
        .filter(|part| text(part, "/kind") == Some("injected"))
        .filter_map(|part| text(part, "/body"))
        .map(str::len)
        .sum();
    Some(tokens(bytes) as u64)
}

/// The durations of one session's own judge calls in a page of
/// `engine::traces::spans`. Its filters pick whole traces, so the page also
/// holds the other spans of those traces, and a child session's own.
pub fn judge_call_ms(page: &Value, session_id: &str) -> Vec<f64> {
    let session = json!(["iii.session.id", session_id]);
    page.get("spans")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|span| text(span, "/name") == Some(JUDGE_CALL_SPAN))
        .filter(|span| {
            span.get("attributes")
                .and_then(Value::as_array)
                .is_some_and(|attributes| attributes.contains(&session))
        })
        .filter_map(|span| {
            let start = span.get("start_time_unix_nano")?.as_u64()?;
            // A span still open ends at 0.
            let end = span.get("end_time_unix_nano")?.as_u64()?;
            (end >= start).then(|| (end - start) as f64 / 1e6)
        })
        .collect()
}

/// Bytes / 4, rounded up: the estimate the suite uses where nothing counted.
fn tokens(bytes: usize) -> f64 {
    bytes.div_ceil(4) as f64
}

fn messages(transcript: Option<&Value>) -> Option<&[Value]> {
    transcript?.get("messages")?.as_array().map(Vec::as_slice)
}

fn content(entry: &Value) -> impl Iterator<Item = &Value> {
    entry
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("function_call"))
}

/// A call as the Harness plans it: the target the `agent_trigger` wrapper
/// names and its payload, without the wrapper's `description`. A wrapper
/// with no target stays `agent_trigger`.
fn planned(block: &Value) -> Option<(&str, Value)> {
    let function_id = block.get("function_id")?.as_str()?;
    let arguments = block.get("arguments").cloned().unwrap_or(Value::Null);
    let target = block.pointer("/arguments/function").and_then(Value::as_str);
    let (Some(target), "agent_trigger") = (target, function_id) else {
        return Some((function_id, arguments));
    };
    let payload = match arguments.get("payload") {
        Some(Value::String(payload)) => {
            serde_json::from_str(payload).unwrap_or_else(|_| Value::String(payload.clone()))
        }
        Some(payload) => payload.clone(),
        // The Harness hoists a flattened payload from beside `function`.
        None => {
            let mut hoisted = arguments.as_object().cloned().unwrap_or_default();
            hoisted.remove("function");
            hoisted.remove("description");
            Value::Object(hoisted)
        }
    };
    Some((target, payload))
}

/// The target's own error content, as the breaker digests it: before the
/// Harness appends its `[harness]` notes to the result.
fn target_error(entry: &Value) -> Value {
    Value::Array(
        entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| !text(block, "/text").is_some_and(|t| t.starts_with("[harness]")))
            .cloned()
            .collect(),
    )
}

fn is_reconciled(entry: &Value) -> bool {
    entry
        .pointer("/origin/reconciled")
        .and_then(Value::as_array)
        .is_some_and(|changes| !changes.is_empty())
}

/// Nearest rank.
fn p95(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let rank = (values.len() * 95).div_ceil(100);
    values.get(rank.checked_sub(1)?).copied()
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::wire::{Observed, SessionMetricsPayload, SessionUsageTotals};

    fn transcript(messages: Value) -> Value {
        json!({ "messages": messages })
    }

    fn call(id: &str, target: &str, payload: Value) -> Value {
        json!({ "type": "function_call", "id": id, "function_id": "agent_trigger",
                "arguments": { "function": target, "payload": payload } })
    }

    fn assistant(turn: &str, timestamp: u64, calls: Vec<Value>) -> Value {
        json!({ "entry_id": format!("e_{turn}_assistant"), "origin": { "turn_id": turn },
                "message": { "role": "assistant", "timestamp": timestamp, "content": calls } })
    }

    fn result(turn: &str, call_id: &str, timestamp: u64, details: Value, is_error: bool) -> Value {
        json!({ "entry_id": format!("e_{turn}_call_{call_id}"), "origin": { "turn_id": turn },
                "message": { "role": "function_result", "function_call_id": call_id,
                             "timestamp": timestamp, "is_error": is_error, "details": details,
                             "content": [{ "type": "text", "text": "x" }] } })
    }

    fn failed(turn: &str, call_id: &str, timestamp: u64, error: &str) -> Value {
        let mut entry = result(
            turn,
            call_id,
            timestamp,
            json!({ "error": { "code": "invocation_failed", "message": error } }),
            true,
        );
        entry["message"]["content"] = json!([{ "type": "text", "text": error }]);
        entry
    }

    fn notice(kind: &str, text: &str) -> Value {
        json!({ "custom": { "custom_type": "model_notice",
                            "data": { "kind": kind, "text": text } } })
    }

    fn run(messages: Option<Value>) -> E2eRunReport {
        let mut report = E2eRunReport::new("run".into(), "a".into(), 1, "root".into(), "p".into());
        report.transcript = messages.map(transcript);
        report
    }

    #[test]
    fn a_clean_transcript_counts_zero_and_leaves_out_what_it_cannot_bound() {
        let measured = measure(&run(Some(json!([]))));

        assert_eq!(measured["registry_notices"], 0.0);
        assert_eq!(measured["repeated_failure"], 0.0);
        assert_eq!(measured["sweep_resolved_calls"], 0.0);
        assert_eq!(measured["arguments_truncated_invalid_json"], 0.0);
        assert_eq!(measured["hook_guidance_tokens"], 0.0);
        // No traces, no judge call, no prompt parts, no delivery count.
        for key in NOT_FROM_TRANSCRIPT {
            assert!(!measured.contains_key(*key), "{key}");
        }
        assert!(measure(&run(None)).is_empty());
    }

    #[test]
    fn what_the_suite_read_beside_the_transcript_is_charted_only_when_read() {
        let mut read = run(None);
        read.duplicate_entry_events = Some(2);
        read.injected_prompt_tokens = Some(1_941);
        read.metrics = Some(Observed::from_normalized(SessionMetricsPayload {
            root_session_id: "root".into(),
            complete: true,
            totals: SessionUsageTotals::default(),
            by_session: Vec::new(),
            traces: Some(json!({ "trace_count": 2, "span_count": 40, "error_span_count": 3 })),
        }));

        let measured = measure(&read);

        assert_eq!(measured["duplicate_entry_events"], 2.0);
        assert_eq!(measured["injected_prompt_tokens"], 1_941.0);
        assert_eq!(
            (measured["trace_spans"], measured["error_spans"]),
            (40.0, 3.0)
        );
        assert!(!measured.contains_key("hook_guidance_tokens"));
    }

    #[test]
    fn notices_refusals_and_repeated_info_calls_are_counted() {
        let messages = json!([
            notice("registry-changed", "functions changed"),
            // The text hooks appended: 9 and 8 bytes, 5 tokens.
            notice("hook", "Use fp:: "),
            notice("registry-changed", "functions changed"),
            notice("hook", "Be brief"),
            assistant(
                "t1",
                0,
                vec![
                    call("c1", FUNCTIONS_INFO, json!({ "function_ids": ["a::b"] })),
                    call("c2", FUNCTIONS_INFO, json!({ "function_ids": ["a::b"] })),
                    call("c3", FUNCTIONS_INFO, json!({ "function_ids": ["c::d"] })),
                ]
            ),
            result(
                "t1",
                "c4",
                1,
                json!({ "error": "arguments_truncated", "cause": "invalid_json" }),
                true
            ),
            result(
                "t1",
                "c5",
                2,
                json!({ "error": "arguments_truncated", "cause": "incomplete_json" }),
                true
            ),
            result(
                "t1",
                "c6",
                3,
                json!({ "error": "agent_trigger_no_target" }),
                true
            ),
            result(
                "t1",
                "c7",
                4,
                json!({ "error": "repeated_failure", "count": 2 }),
                true
            ),
            result("t1", "c8", 5, json!({ "error": "pending_timeout" }), true),
        ]);

        let measured = measure(&run(Some(messages)));

        let pick = |key: &str| measured[key];
        assert_eq!((pick("registry_notices"), pick("hook_notices")), (2.0, 2.0));
        assert_eq!(pick("hook_guidance_tokens"), 5.0);
        assert_eq!(pick("functions_info_repeats"), 1.0);
        assert_eq!(pick("arguments_truncated_invalid_json"), 1.0);
        assert_eq!(pick("arguments_truncated_incomplete_json"), 1.0);
        assert_eq!(pick("arguments_truncated_max_output_tokens"), 0.0);
        assert_eq!(pick("agent_trigger_no_target"), 1.0);
        assert_eq!(pick("repeated_failure"), 1.0);
        assert_eq!(pick("sweep_resolved_calls"), 1.0);
    }

    #[test]
    fn only_the_prompt_parts_hooks_inject_are_counted() {
        let preview = json!({ "parts": [
            { "kind": "built_in", "name": "embedded harness default", "body": "x".repeat(400) },
            { "kind": "runtime", "body": "x".repeat(40) },
            { "kind": "injected", "name": "browser", "body": "x".repeat(10) },
            { "kind": "injected", "name": "fp", "body": "x".repeat(5) },
        ] });

        assert_eq!(injected_prompt_tokens(&preview), Some(4));
        assert_eq!(injected_prompt_tokens(&json!({ "parts": [] })), Some(0));
        assert_eq!(injected_prompt_tokens(&json!({})), None);
    }

    #[test]
    fn reconciliation_is_timed_by_the_sessions_own_judge_calls() {
        let span = |name: &str, session: &str, start: u64, end: u64| {
            json!({ "name": name, "start_time_unix_nano": start, "end_time_unix_nano": end,
                    "attributes": [["function_id", "judge::evaluate"],
                                   ["iii.session.id", session]] })
        };
        let page = json!({ "total": 5, "spans": [
            span(JUDGE_CALL_SPAN, "root", 1_000_000, 1_800_000_000),
            // The same trace's other spans, a child's own judge call, and
            // a call still open.
            span("execute judge::evaluate", "root", 0, 2_000_000_000),
            span(JUDGE_CALL_SPAN, "child", 0, 9_000_000_000),
            span(JUDGE_CALL_SPAN, "root", 5_000_000, 0),
            span(JUDGE_CALL_SPAN, "root", 0, 2_600_000_000),
        ] });

        assert_eq!(judge_call_ms(&page, "root"), vec![1_799.0, 2_600.0]);

        let mut timed = run(Some(json!([])));
        timed.reconciliation_judge_ms = Some(vec![1_799.0, 2_600.0, 40.0]);
        assert_eq!(measure(&timed)["reconciled_p95_ms"], 2_600.0);
        // Every reconciliation ran in process: nothing to time.
        timed.reconciliation_judge_ms = Some(Vec::new());
        assert!(!measure(&timed).contains_key("reconciled_p95_ms"));
    }

    fn passed(messages: Value) -> E2eRunReport {
        let mut report = run(Some(messages));
        report.status = RunStatus::Passed;
        report
    }

    #[test]
    fn a_third_identical_failure_run_in_one_turn_breaks_the_breaker() {
        // The breaker digests the target's error before the Harness's notes.
        let mut noted = failed("t1", "c3", 5, "boom");
        noted["message"]["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "type": "text", "text": "[harness] These arguments do not match" }));
        let report = passed(json!([
            assistant("t1", 0, vec![call("c1", "a::b", json!({ "x": 1 }))]),
            failed("t1", "c1", 1, "boom"),
            assistant("t1", 2, vec![call("c2", "a::b", json!({ "x": 1 }))]),
            failed("t1", "c2", 3, "boom"),
            assistant("t1", 4, vec![call("c3", "a::b", json!({ "x": 1 }))]),
            noted,
        ]));

        let violations = repeated_failures(&report);

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].invariant,
            HarnessInvariant::RepeatedFailureExecuted
        );
        assert!(
            violations[0].detail.contains("a::b"),
            "{}",
            violations[0].detail
        );
    }

    #[test]
    fn what_resets_the_breaker_or_never_reaches_it_is_not_a_repeat() {
        let same = |id: &str| call(id, "a::b", json!({ "x": 1 }));
        let report = passed(json!([
            // A success between them restarts the count, like the Harness's.
            assistant(
                "t1",
                0,
                vec![same("c1"), same("c2"), same("c3"), same("c4"), same("c5")]
            ),
            failed("t1", "c1", 1, "boom"),
            failed("t1", "c2", 2, "boom"),
            result("t1", "c3", 3, json!({}), false),
            failed("t1", "c4", 4, "boom"),
            failed("t1", "c5", 5, "boom"),
            // A different error restarts it too.
            assistant("t2", 6, vec![same("d1"), same("d2"), same("d3")]),
            failed("t2", "d1", 7, "boom"),
            failed("t2", "d2", 8, "boom"),
            failed("t2", "d3", 9, "other"),
            // The breaker's own answer did not run the call, and the sweep
            // resolves a pending call outside it.
            assistant("t2", 10, vec![same("d4"), same("d5"), same("d6")]),
            result("t2", "d4", 11, json!({ "error": "repeated_failure" }), true),
            result("t2", "d5", 11, json!({ "error": "pending_timeout" }), true),
            result("t2", "d6", 11, json!({ "error": "pending_timeout" }), true),
            // Another turn starts over.
            assistant("t3", 12, vec![same("e1"), same("e2")]),
            failed("t3", "e1", 13, "other"),
            failed("t3", "e2", 14, "other"),
        ]));

        assert!(repeated_failures(&report).is_empty());
        let mut stopped = report.clone();
        stopped.status = RunStatus::ResourceLimit;
        assert!(repeated_failures(&stopped).is_empty());
    }

    #[test]
    fn calls_the_breaker_never_notes_are_not_repeats() {
        // Spawn guards, ask refusals and submit_result validation answer in
        // the turn loop before the breaker notes a result; a cut-off call's
        // result is not the target's either.
        let spawn = |id: &str| call(id, "harness::spawn", json!({ "prompt": "p" }));
        let ask = |id: &str| call(id, "harness::ask", json!({ "question": "q" }));
        let submit = |id: &str| {
            json!({ "type": "function_call", "id": id, "function_id": "submit_result",
                    "arguments": { "answer": 1 } })
        };
        let same = |id: &str| call(id, "a::b", json!({ "x": 1 }));
        let refused = |turn: &str, id: &str, details: Value| {
            let mut entry = result(turn, id, 1, details, true);
            entry["message"]["content"] = json!([{ "type": "text", "text": "refused" }]);
            entry
        };
        // Worded like the target's error, so only the path tells them apart.
        let cut_off = |turn: &str, id: &str, details: Value| {
            let mut entry = result(turn, id, 1, details, true);
            entry["message"]["content"] = json!([{ "type": "text", "text": "boom" }]);
            entry
        };
        let depth = json!({ "error": "harness/spawn_depth_exceeded", "message": "too deep" });
        let report = passed(json!([
            assistant("t1", 0, vec![spawn("s1"), spawn("s2"), spawn("s3")]),
            refused("t1", "s1", depth.clone()),
            refused("t1", "s2", depth.clone()),
            refused("t1", "s3", depth),
            assistant("t1", 2, vec![ask("a1"), ask("a2"), ask("a3")]),
            refused("t1", "a1", Value::Null),
            refused("t1", "a2", Value::Null),
            refused("t1", "a3", Value::Null),
            assistant("t1", 3, vec![submit("r1"), submit("r2"), submit("r3")]),
            refused("t1", "r1", Value::Null),
            refused("t1", "r2", Value::Null),
            refused("t1", "r3", Value::Null),
            assistant("t1", 4, vec![same("c1"), same("c2"), same("c3")]),
            failed("t1", "c1", 5, "boom"),
            failed("t1", "c2", 6, "boom"),
            cut_off("t1", "c3", json!({ "error": "interrupted" })),
            assistant("t2", 7, vec![same("d1"), same("d2"), same("d3")]),
            failed("t2", "d1", 8, "boom"),
            failed("t2", "d2", 9, "boom"),
            cut_off(
                "t2",
                "d3",
                json!({ "error": "cancelled", "cancelled_by": "user" })
            ),
        ]));

        assert_eq!(repeated_failures(&report), Vec::new());
    }

    #[test]
    fn the_breaker_keys_a_call_on_its_target_and_payload_not_its_description() {
        let described = |id: &str, description: &str| {
            json!({ "type": "function_call", "id": id, "function_id": "agent_trigger",
                    "arguments": { "function": "a::b", "description": description,
                                   "payload": { "x": 1 } } })
        };
        let report = passed(json!([
            assistant("t1", 0, vec![described("c1", "Trying a::b")]),
            failed("t1", "c1", 1, "boom"),
            assistant("t1", 2, vec![described("c2", "Retrying a::b")]),
            failed("t1", "c2", 3, "boom"),
            assistant("t1", 4, vec![described("c3", "Once more")]),
            failed("t1", "c3", 5, "boom"),
        ]));

        assert_eq!(repeated_failures(&report).len(), 1);
    }

    #[test]
    fn slow_reconciliations_are_flagged_past_two_and_a_half_seconds() {
        let mut slow = run(None);
        slow.reconciliation_judge_ms = Some(vec![120.0, 2_600.0]);
        let mut fast = run(None);
        fast.reconciliation_judge_ms = Some(vec![120.0, 2_500.0]);

        let flag = slow_reconciled_flag(&slow).expect("slow reconciliation flagged");
        assert_eq!(flag.kind, AuditFlagKind::SlowReconciledCalls);
        assert!(flag.summary.contains("2600 ms"), "{}", flag.summary);
        assert!(slow_reconciled_flag(&fast).is_none());
        assert!(slow_reconciled_flag(&run(None)).is_none());
    }
}
