//! Harness mechanisms whose numbers depend on the model or the environment,
//! so Trends charts them instead of gating on them, read from the root
//! session's transcript and the session-tree metrics the suite already
//! captured. One gate rides along: the repeated-failure breaker.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::audit::{AuditFlag, AuditFlagKind, AuditFlagSource, AuditSeverity};
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::{E2eRunReport, RunStatus};
use crate::wire::SessionMetricsResponse;

/// What Trends charts per run, by the name its `<name>_mean` takes.
pub const TREND_KEYS: &[&str] = &[
    "trace_spans",
    "error_spans",
    "registry_notices",
    "hook_notices",
    "hook_guidance_tokens",
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

/// The Harness's `REPEATED_FAILURE_LIMIT`: identical failures of one call a
/// turn runs before the next identical call is answered locally.
const REPEATED_FAILURE_LIMIT: u32 = 2;

/// Above it a run's reconciled calls are flagged as slow.
const RECONCILED_P95_ALERT_MS: f64 = 2_500.0;

/// Answers the Harness gives a call without running its target, or through
/// the resolve path, which does not note results: they never reach the
/// breaker.
const LOCAL_REFUSALS: &[&str] = &[
    "repeated_failure",
    "pending_timeout",
    "arguments_truncated",
    "agent_trigger_no_target",
    "policy_denied",
    "hook_denied",
];

const FUNCTIONS_INFO: &str = "engine::functions::info";

/// A key is absent when its evidence is: no transcript, no traces, no
/// reconciled call whose latency the transcript bounds, no count of the root
/// session's `session::message-added` deliveries. A count over present
/// evidence is there even at zero, so a mean never skips a clean run.
pub fn measure(
    transcript: Option<&Value>,
    metrics: Option<&SessionMetricsResponse>,
    duplicate_entry_events: Option<u64>,
) -> BTreeMap<String, f64> {
    let mut measured = BTreeMap::new();
    if let Some(count) = duplicate_entry_events {
        measured.insert("duplicate_entry_events".into(), count as f64);
    }
    if let Some(traces) = metrics.and_then(|metrics| metrics.traces.as_ref()) {
        for (key, field) in [
            ("trace_spans", "span_count"),
            ("error_spans", "error_span_count"),
        ] {
            if let Some(count) = traces.get(field).and_then(Value::as_u64) {
                measured.insert(key.to_string(), count as f64);
            }
        }
    }
    let hook_guidance = metrics
        .and_then(|metrics| metrics.by_session.iter().find(|session| session.depth == 0))
        .and_then(|root| root.context.as_ref())
        .and_then(|context| context.pointer("/categories/hook_guidance"))
        .and_then(Value::as_u64);
    if let Some(tokens) = hook_guidance {
        measured.insert("hook_guidance_tokens".into(), tokens as f64);
    }
    let Some(messages) = messages(transcript) else {
        return measured;
    };
    for key in TREND_KEYS {
        if ![
            "trace_spans",
            "error_spans",
            "hook_guidance_tokens",
            "reconciled_p95_ms",
            "duplicate_entry_events",
        ]
        .contains(key)
        {
            measured.insert(key.to_string(), 0.0);
        }
    }
    let mut count = |key: String| *measured.entry(key).or_insert(0.0) += 1.0;
    let mut info_payloads = Vec::new();
    for entry in messages {
        match text(entry, "/custom/data/kind") {
            Some("registry-changed") => count("registry_notices".into()),
            Some("hook") => count("hook_notices".into()),
            _ => {}
        }
        for (function_id, payload) in calls(entry) {
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
    if let Some(p95) = p95(reconciled_latencies(messages)) {
        measured.insert("reconciled_p95_ms".into(), p95);
    }
    measured
}

/// The breaker answers the third identical failure of a call in a turn
/// locally, so a passed run whose turn ran a call into the same error more
/// than twice ran past it. Like the Harness, a success clears a call's count
/// and a different error restarts it.
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
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    keys.insert(id, block_key(block));
                }
            }
            continue;
        }
        if text(entry, "/message/role") != Some("function_result")
            || text(entry, "/message/details/error")
                .is_some_and(|code| LOCAL_REFUSALS.contains(&code))
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

/// Advisory: the p95 of a run's reconciled-call latencies past 2.5 s.
pub fn slow_reconciled_flag(report: &E2eRunReport) -> Option<AuditFlag> {
    let p95 = p95(reconciled_latencies(messages(report.transcript.as_ref())?))?;
    (p95 > RECONCILED_P95_ALERT_MS).then(|| AuditFlag {
        kind: AuditFlagKind::SlowReconciledCalls,
        severity: AuditSeverity::Warning,
        source: AuditFlagSource::Deterministic,
        summary: format!("reconciled calls took {p95} ms at the p95, past 2500 ms"),
        evidence: Vec::new(),
        confidence: None,
    })
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

/// Each call of an assistant entry as the target it names and its payload,
/// through the `agent_trigger` wrapper when it is one.
fn calls(entry: &Value) -> impl Iterator<Item = (&str, Option<&Value>)> {
    content(entry).filter_map(|block| {
        let function_id = block.get("function_id")?.as_str()?;
        let arguments = block.get("arguments");
        Some(
            match arguments
                .and_then(|a| a.get("function"))
                .and_then(Value::as_str)
            {
                Some(target) if function_id == "agent_trigger" => {
                    (target, arguments.and_then(|a| a.get("payload")))
                }
                _ => (function_id, arguments),
            },
        )
    })
}

fn block_key(block: &Value) -> String {
    let function_id = block
        .get("function_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!(
        "{function_id} {}",
        block.get("arguments").unwrap_or(&Value::Null)
    )
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

/// The calls of a step run one after another and each result is stamped when
/// appended, so a call after a sibling took the time since that sibling's
/// result. The first call of a step is not timed: its assistant entry is
/// stamped when the generation starts.
/// shortcut: first calls go untimed; read their dispatch span from the engine
/// traces when Trends needs every reconciled call.
fn reconciled_latencies(messages: &[Value]) -> Vec<f64> {
    let mut previous = None;
    let mut latencies = Vec::new();
    for entry in messages {
        let Some(timestamp) = entry.pointer("/message/timestamp").and_then(Value::as_u64) else {
            continue;
        };
        let role = text(entry, "/message/role");
        if role == Some("function_result") && is_reconciled(entry) {
            if let Some((Some("function_result"), before)) = previous {
                latencies.push(timestamp.saturating_sub(before) as f64);
            }
        }
        previous = Some((role, timestamp));
    }
    latencies
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
    use crate::wire::{Observed, SessionMetricsPayload, SessionUsage, SessionUsageTotals};

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

    fn notice(kind: &str) -> Value {
        json!({ "custom": { "custom_type": "model_notice", "data": { "kind": kind } } })
    }

    fn metrics(traces: Option<Value>, context: Option<Value>) -> SessionMetricsResponse {
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
                context,
            }],
            traces,
        })
    }

    #[test]
    fn a_clean_transcript_counts_zero_and_leaves_out_what_it_cannot_bound() {
        let measured = measure(Some(&transcript(json!([]))), None, None);

        assert_eq!(measured["registry_notices"], 0.0);
        assert_eq!(measured["repeated_failure"], 0.0);
        assert_eq!(measured["sweep_resolved_calls"], 0.0);
        assert_eq!(measured["arguments_truncated_invalid_json"], 0.0);
        // No traces, no reconciled call, no context snapshot: unmeasured.
        for key in ["trace_spans", "reconciled_p95_ms", "hook_guidance_tokens"] {
            assert!(!measured.contains_key(key), "{key}");
        }
        assert!(measure(None, None, None).is_empty());
    }

    #[test]
    fn duplicate_entry_events_are_charted_only_when_they_were_counted() {
        let measured = measure(None, None, Some(2));

        assert_eq!(measured["duplicate_entry_events"], 2.0);
        assert!(!measure(None, None, None).contains_key("duplicate_entry_events"));
    }

    #[test]
    fn notices_refusals_and_repeated_info_calls_are_counted() {
        let messages = json!([
            notice("registry-changed"),
            notice("hook"),
            notice("registry-changed"),
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
        let traces = json!({ "trace_count": 2, "span_count": 40, "error_span_count": 3 });
        let context = json!({ "categories": { "hook_guidance": 928 } });

        let measured = measure(
            Some(&transcript(messages)),
            Some(&metrics(Some(traces), Some(context))),
            None,
        );

        let pick = |key: &str| measured[key];
        assert_eq!((pick("registry_notices"), pick("hook_notices")), (2.0, 1.0));
        assert_eq!(pick("functions_info_repeats"), 1.0);
        assert_eq!(pick("arguments_truncated_invalid_json"), 1.0);
        assert_eq!(pick("arguments_truncated_incomplete_json"), 1.0);
        assert_eq!(pick("arguments_truncated_max_output_tokens"), 0.0);
        assert_eq!(pick("agent_trigger_no_target"), 1.0);
        assert_eq!(pick("repeated_failure"), 1.0);
        assert_eq!(pick("sweep_resolved_calls"), 1.0);
        assert_eq!((pick("trace_spans"), pick("error_spans")), (40.0, 3.0));
        assert_eq!(pick("hook_guidance_tokens"), 928.0);
    }

    fn reconciled(turn: &str, call_id: &str, timestamp: u64) -> Value {
        let mut entry = result(turn, call_id, timestamp, json!({}), false);
        entry["origin"]["reconciled"] = json!([{ "kind": "renamed", "path": "/a" }]);
        entry
    }

    #[test]
    fn a_reconciled_call_is_timed_only_after_a_sibling_result() {
        // The assistant entry is stamped when its generation starts, so the
        // first call of a step is not timed; a later one runs after the
        // previous result.
        let messages = json!([
            assistant("t1", 1_000, vec![]),
            reconciled("t1", "c1", 4_000),
            reconciled("t1", "c2", 4_100),
            result("t1", "c3", 4_200, json!({}), false),
            reconciled("t1", "c4", 7_200),
        ]);

        let measured = measure(Some(&transcript(messages)), None, None);

        assert_eq!(measured["reconciled_calls"], 3.0);
        assert_eq!(measured["reconciled_p95_ms"], 3_000.0);
    }

    fn passed(messages: Value) -> E2eRunReport {
        let mut report = E2eRunReport::new("run".into(), "a".into(), 1, "root".into(), "p".into());
        report.status = RunStatus::Passed;
        report.transcript = Some(transcript(messages));
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
    fn slow_reconciled_calls_are_flagged_past_two_and_a_half_seconds() {
        let slow = passed(json!([
            assistant("t1", 0, vec![]),
            result("t1", "c0", 100, json!({}), false),
            reconciled("t1", "c1", 2_700),
        ]));
        let fast = passed(json!([
            assistant("t1", 0, vec![]),
            result("t1", "c0", 100, json!({}), false),
            reconciled("t1", "c1", 2_500),
        ]));

        let flag = slow_reconciled_flag(&slow).expect("slow reconciled calls flagged");
        assert_eq!(flag.kind, AuditFlagKind::SlowReconciledCalls);
        assert!(flag.summary.contains("2600 ms"), "{}", flag.summary);
        assert!(slow_reconciled_flag(&fast).is_none());
    }
}
