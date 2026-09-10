use std::path::Path;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use super::CriterionAward;
use crate::context::E2eContext;

#[derive(Debug, Clone, PartialEq)]
pub struct ObservedFunctionCall {
    pub function_id: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservedFunctionInvocation {
    pub call_id: Option<String>,
    pub call: ObservedFunctionCall,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservedFunctionOutcome {
    pub ordinal: usize,
    pub call_id: Option<String>,
    pub function_id: String,
    pub arguments: Value,
    pub is_error: Option<bool>,
    pub error_code: Option<String>,
    pub details: Option<Value>,
}

/// Returns whether a function call is contract discovery rather than product work.
///
/// The directory alias is intentionally narrow: only the engine function-info
/// endpoint is discovery. Other `directory::*` calls remain observable work.
pub fn is_contract_discovery(function_id: &str) -> bool {
    function_id.starts_with("engine::functions::")
        || function_id == "directory::engine::functions::info"
}

pub fn final_response(transcript: &Value) -> String {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .rev()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .find(|response| !response.trim().is_empty())
        .unwrap_or_default()
}

pub fn function_calls(transcript: &Value) -> Vec<ObservedFunctionCall> {
    function_invocations(transcript)
        .into_iter()
        .map(|invocation| invocation.call)
        .collect()
}

pub fn function_invocations(transcript: &Value) -> Vec<ObservedFunctionInvocation> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .flat_map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("function_call"))
        .filter_map(normalize_invocation)
        .collect()
}

fn normalize_invocation(block: &Value) -> Option<ObservedFunctionInvocation> {
    let call_id = block.get("id").and_then(Value::as_str).map(str::to_owned);
    let function_id = block.get("function_id")?.as_str()?;
    let arguments = block.get("arguments").cloned().unwrap_or_else(|| json!({}));
    if function_id == "agent_trigger" {
        return Some(ObservedFunctionInvocation {
            call_id,
            call: ObservedFunctionCall {
                function_id: arguments.get("function")?.as_str()?.to_string(),
                arguments: arguments
                    .get("payload")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            },
        });
    }
    Some(ObservedFunctionInvocation {
        call_id,
        call: ObservedFunctionCall {
            function_id: function_id.to_string(),
            arguments,
        },
    })
}

pub fn function_result<'a>(
    transcript: &'a Value,
    invocation: &ObservedFunctionInvocation,
) -> Option<&'a Value> {
    let call_id = invocation.call_id.as_deref()?;
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .find(|message| {
            message.get("role").and_then(Value::as_str) == Some("function_result")
                && message.get("function_call_id").and_then(Value::as_str) == Some(call_id)
                && message.get("function_id").and_then(Value::as_str)
                    == Some(invocation.call.function_id.as_str())
                && message.get("is_error").and_then(Value::as_bool) == Some(false)
        })
}

/// Function calls paired with their result when one was durably captured.
/// Unlike `function_result`, this deliberately retains failed outcomes so
/// scenario evaluators can distinguish a recovered contract error from an
/// unobserved or silently retried call.
pub fn function_outcomes(transcript: &Value) -> Vec<ObservedFunctionOutcome> {
    function_invocations(transcript)
        .into_iter()
        .enumerate()
        .map(|(ordinal, invocation)| {
            let result = invocation.call_id.as_deref().and_then(|call_id| {
                transcript
                    .get("messages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry.get("message"))
                    .find(|message| {
                        message.get("role").and_then(Value::as_str) == Some("function_result")
                            && message.get("function_call_id").and_then(Value::as_str)
                                == Some(call_id)
                            && message.get("function_id").and_then(Value::as_str)
                                == Some(invocation.call.function_id.as_str())
                    })
            });
            ObservedFunctionOutcome {
                ordinal,
                call_id: invocation.call_id,
                function_id: invocation.call.function_id,
                arguments: invocation.call.arguments,
                is_error: result
                    .and_then(|value| value.get("is_error"))
                    .and_then(Value::as_bool),
                error_code: result
                    .and_then(|value| {
                        value
                            .get("error_code")
                            .or_else(|| value.pointer("/details/code"))
                            .or_else(|| value.pointer("/details/error/code"))
                    })
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                details: result.and_then(|value| value.get("details")).cloned(),
            }
        })
        .collect()
}

/// Failed trigger-info probes against the fallback namespace are discovery,
/// not failed product work. Only correlated, fully identified failures count.
pub fn identified_discovery_errors(transcript: &Value) -> u64 {
    function_outcomes(transcript)
        .iter()
        .filter(|outcome| {
            outcome.function_id == "engine::triggers::info"
                && outcome
                    .arguments
                    .get("namespace")
                    .is_none_or(|namespace| namespace.as_str() == Some("default"))
                && outcome.is_error == Some(true)
                && outcome.error_code.as_deref() == Some("NOT_FOUND")
        })
        .count() as u64
}

pub fn operational_function_errors(total: u64, identified_discovery: u64) -> u64 {
    total.checked_sub(identified_discovery).unwrap_or(total)
}

pub fn state_value(response: Value) -> Value {
    match response {
        Value::Object(mut object)
            if object.get("ok").and_then(Value::as_bool) == Some(true)
                && object.contains_key("value") =>
        {
            object.remove("value").unwrap_or(Value::Null)
        }
        response => response,
    }
}

pub fn requested_once(arguments: &Value) -> bool {
    arguments.get("once").and_then(Value::as_bool) == Some(true)
        || arguments
            .pointer("/lifecycle/once")
            .and_then(Value::as_bool)
            == Some(true)
}

pub fn is_wake_registration(arguments: &Value) -> bool {
    arguments.get("function_id").is_none_or(Value::is_null)
        && arguments.get("target").is_none_or(|target| {
            target.is_null()
                || target.get("function_id").and_then(Value::as_str) == Some("harness::send")
        })
}

/// Whether ANY text block in the transcript contains `needle` — for spotting
/// error results and machine messages regardless of which entry carried them.
pub fn transcript_contains(transcript: &Value, needle: &str) -> bool {
    fn walk(value: &Value, needle: &str) -> bool {
        match value {
            Value::String(text) => text.contains(needle),
            Value::Array(items) => items.iter().any(|item| walk(item, needle)),
            Value::Object(map) => map.values().any(|item| walk(item, needle)),
            _ => false,
        }
    }
    walk(transcript, needle)
}

/// Validation nudges the harness appended to this transcript — the
/// re-prompts a `harness::hook::post-turn` validator (or the output
/// contract) produced. Recognized by the durable entry id
/// (`e_<turn>_nudge_<n>`) or the `validation` origin flag.
pub fn validation_nudges(transcript: &Value) -> usize {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry
                .get("entry_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.contains("_nudge_"))
                || entry
                    .pointer("/origin/validation")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .count()
}

pub fn trigger_fired_records(transcript: &Value) -> Vec<&Value> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("custom"))
        .filter(|custom| custom.get("custom_type").and_then(Value::as_str) == Some("trigger_fired"))
        .filter_map(|custom| custom.get("data"))
        .collect()
}

pub async fn active_binding_count(context: &E2eContext, session_id: &str) -> anyhow::Result<usize> {
    Ok(context
        .trigger_value(
            "harness::triggers::list",
            json!({ "session_id": session_id }),
        )
        .await?
        .get("subscriptions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(usize::MAX))
}

/// Score a fixture-driven scenario from its atomic observations. `metrics`
/// is the catalog (`id`, `weight`, `measurement`); `validation.observations`
/// holds one `measured` item per metric with `value` (binary) or
/// `numerator`/`denominator` (ratio, `value` deciding a zero denominator).
/// Points are `round(weight × value)`. A missing, duplicate, unavailable or
/// malformed observation fails the whole evaluation: a validator problem is
/// unavailability, never a silent zero.
pub fn atomic_awards(metrics: &[Value], validation: &Value) -> Result<Vec<CriterionAward>> {
    let items = validation["observations"]
        .as_array()
        .context("validation observations missing")?;
    if items.len() != metrics.len() {
        bail!("validation incomplete: {}", validation["error"]);
    }
    metrics
        .iter()
        .map(|metric| {
            let matches: Vec<_> = items
                .iter()
                .filter(|item| item["id"] == metric["id"])
                .collect();
            if matches.len() != 1 {
                bail!("missing or duplicate metric {}", metric["id"]);
            }
            let item = matches[0];
            if item["status"] != "measured" {
                bail!("{} is {}: {}", metric["id"], item["status"], item["reason"]);
            }
            let value = if metric["measurement"] == "binary" {
                let value = item["value"]
                    .as_u64()
                    .filter(|v| *v <= 1)
                    .context("binary value must be 0 or 1")?;
                value as f64
            } else {
                let numerator = item["numerator"]
                    .as_u64()
                    .context("ratio numerator missing")?;
                let denominator = item["denominator"]
                    .as_u64()
                    .context("ratio denominator missing")?;
                if numerator > denominator {
                    bail!("ratio numerator exceeds denominator");
                }
                if denominator == 0 {
                    item["value"]
                        .as_u64()
                        .filter(|v| *v <= 1)
                        .context("zero-denominator ratio value must be 0 or 1")?
                        as f64
                } else {
                    numerator as f64 / denominator as f64
                }
            };
            Ok(CriterionAward {
                id: metric["id"].as_str().unwrap().into(),
                awarded: Some((metric["weight"].as_u64().unwrap() as f64 * value).round() as u8),
                reason: item.to_string(),
            })
        })
        .collect()
}

/// Embed the evidence a scenario left under `directory` so the report does
/// not depend on the workspace surviving: files directly under `directory`,
/// plus everything below the top-level entries named in `roots` (a root may
/// be nested, e.g. `workspace/output`). Text is embedded as UTF-8, anything
/// else as base64; once half of the capture cap is spent the rest is listed
/// under `omitted_files` instead of being read.
pub fn evidence_bundle(directory: &Path, roots: &[&str]) -> Result<Value> {
    let mut pending = vec![directory.to_path_buf()];
    let mut files = serde_json::Map::new();
    let mut omitted = Vec::new();
    let mut bytes = 0;
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let relative = path.strip_prefix(directory)?;
            if kind.is_dir() {
                if folder != directory || roots.iter().any(|root| Path::new(root) == relative) {
                    pending.push(path);
                } else {
                    for root in roots {
                        let root = Path::new(root);
                        if root != relative && root.starts_with(relative) {
                            let nested = directory.join(root);
                            if nested.is_dir() {
                                pending.push(nested);
                            }
                        }
                    }
                }
            } else if kind.is_file() {
                let name = path
                    .strip_prefix(directory.parent().context("evidence parent")?)?
                    .to_string_lossy()
                    .into_owned();
                let size = entry.metadata()?.len();
                if bytes + size > crate::asset::DEFAULT_MAX_CAPTURE_BYTES / 2 {
                    omitted.push(json!({"path":name,"reason":"capture_size_limit"}));
                    continue;
                }
                let data = std::fs::read(&path)?;
                let value = match String::from_utf8(data) {
                    Ok(text) => json!({"encoding":"utf8","content":text}),
                    Err(error) => {
                        json!({"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(error.as_bytes())})
                    }
                };
                let encoded_size = serde_json::to_vec(&value)?.len() as u64 + name.len() as u64 + 8;
                if bytes + encoded_size > crate::asset::DEFAULT_MAX_CAPTURE_BYTES / 2 {
                    omitted.push(json!({"path":name,"reason":"capture_size_limit"}));
                    continue;
                }
                bytes += encoded_size;
                files.insert(name, value);
            }
        }
    }
    Ok(json!({"files":files,"omitted_files":omitted}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_agent_trigger_and_native_function_calls() {
        let transcript = json!({
            "messages": [{
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "function_call",
                            "id": "call-state",
                            "function_id": "agent_trigger",
                            "arguments": {
                                "function": "state::set",
                                "payload": { "scope": "s", "key": "k", "value": 1 }
                            }
                        },
                        {
                            "type": "function_call",
                            "id": "call-native",
                            "function_id": "native::call",
                            "arguments": { "value": 2 }
                        }
                    ]
                }
            }]
        });
        let calls = function_calls(&transcript);
        assert_eq!(calls[0].function_id, "state::set");
        assert_eq!(calls[0].arguments["key"], "k");
        assert_eq!(calls[1].function_id, "native::call");
    }

    #[test]
    fn correlates_a_function_result_with_its_call_id() {
        let transcript = json!({
            "messages": [
                {"message": {"role": "assistant", "content": [{
                    "type": "function_call",
                    "id": "call-match",
                    "function_id": "agent_trigger",
                    "arguments": {
                        "function": "state::set",
                        "payload": { "scope": "s", "key": "k", "value": 1 }
                    }
                }]}},
                {"message": {
                    "role": "function_result",
                    "function_call_id": "call-other",
                    "function_id": "state::set",
                    "is_error": false,
                    "details": { "ok": false }
                }},
                {"message": {
                    "role": "function_result",
                    "function_call_id": "call-match",
                    "function_id": "state::set",
                    "is_error": false,
                    "details": { "ok": true }
                }}
            ]
        });
        let invocations = function_invocations(&transcript);
        let result = function_result(&transcript, &invocations[0]).expect("matching result");

        assert_eq!(invocations[0].call_id.as_deref(), Some("call-match"));
        assert_eq!(
            result.pointer("/details/ok").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn outcomes_keep_successes_errors_and_missing_results_in_call_order() {
        let transcript = json!({"messages": [
            {"message": {"role": "assistant", "content": [
                {"type": "function_call", "id": "ok", "function_id": "fixture::read", "arguments": {"id": 1}},
                {"type": "function_call", "id": "bad", "function_id": "fixture::write", "arguments": {"id": 2}},
                {"type": "function_call", "id": "lost", "function_id": "fixture::write", "arguments": {"id": 3}}
            ]}},
            {"message": {"role": "function_result", "function_call_id": "ok", "function_id": "fixture::read", "is_error": false, "details": {"ok": true}}},
            {"message": {"role": "function_result", "function_call_id": "bad", "function_id": "fixture::write", "is_error": true, "details": {"error": {"code": "version_conflict"}}}}
        ]});

        let outcomes = function_outcomes(&transcript);
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].ordinal, 0);
        assert_eq!(outcomes[0].is_error, Some(false));
        assert_eq!(outcomes[1].error_code.as_deref(), Some("version_conflict"));
        assert_eq!(outcomes[2].is_error, None);
    }

    #[test]
    fn excludes_only_identified_default_namespace_trigger_discovery_errors() {
        let transcript = json!({"messages": [
            {"message": {"role": "assistant", "content": [
                {"type": "function_call", "id": "discovery", "function_id": "engine::triggers::info", "arguments": {"id": "timer"}},
                {"type": "function_call", "id": "write", "function_id": "state::set", "arguments": {"scope": "s", "key": "k", "value": 1}},
                {"type": "function_call", "id": "registration", "function_id": "engine::register_trigger", "arguments": {"namespace": "default"}}
            ]}},
            {"message": {"role": "function_result", "function_call_id": "discovery", "function_id": "engine::triggers::info", "is_error": true, "details": {"error": {"code": "NOT_FOUND"}}}},
            {"message": {"role": "function_result", "function_call_id": "write", "function_id": "state::set", "is_error": true, "error_code": "WRITE_FAILED"}},
            {"message": {"role": "function_result", "function_call_id": "registration", "function_id": "engine::register_trigger", "is_error": true, "error_code": "NOT_FOUND"}}
        ]});

        let discovery = identified_discovery_errors(&transcript);
        assert_eq!(discovery, 1);
        assert_eq!(operational_function_errors(1, discovery), 0);
        assert_eq!(operational_function_errors(3, discovery), 2);
    }

    #[test]
    fn malformed_or_uncorrelated_discovery_evidence_does_not_hide_errors() {
        let transcript = json!({"messages": [
            {"message": {"role": "assistant", "content": [
                {"type": "function_call", "id": "missing", "function_id": "engine::triggers::info", "arguments": {"id": "timer", "namespace": "default"}},
                {"type": "function_call", "id": "wrong-namespace", "function_id": "engine::triggers::info", "arguments": {"id": "timer", "namespace": "other"}},
                {"type": "function_call", "id": "missing-code", "function_id": "engine::triggers::info", "arguments": {"id": "timer", "namespace": "default"}}
            ]}},
            {"message": {"role": "function_result", "function_call_id": "wrong-namespace", "function_id": "engine::triggers::info", "is_error": true, "error_code": "NOT_FOUND"}},
            {"message": {"role": "function_result", "function_call_id": "missing-code", "function_id": "engine::triggers::info", "is_error": true}}
        ]});

        assert_eq!(identified_discovery_errors(&transcript), 0);
        assert_eq!(operational_function_errors(3, 0), 3);
        assert_eq!(operational_function_errors(1, 2), 1);
    }

    #[test]
    fn extracts_the_last_nonempty_assistant_response() {
        let transcript = json!({
            "messages": [
                {"message": {"role": "user", "content": [{"type": "text", "text": "no"}]}},
                {"message": {"role": "assistant", "content": [
                    {"type": "text", "text": "yes "},
                    {"type": "text", "text": "indeed"},
                    {"type": "function_call", "function_id": "x", "arguments": {}}
                ]}},
                {"message": {"role": "assistant", "content": [
                    {"type": "function_call", "function_id": "x", "arguments": {}}
                ]}},
            ]
        });
        assert_eq!(final_response(&transcript), "yes indeed");
    }

    #[test]
    fn recognizes_only_the_allowed_contract_discovery_functions() {
        assert!(is_contract_discovery("engine::functions::list"));
        assert!(is_contract_discovery("engine::functions::info"));
        assert!(is_contract_discovery("directory::engine::functions::info"));

        assert!(!is_contract_discovery("directory::engine::functions::list"));
        assert!(!is_contract_discovery("directory::workers::info"));
        assert!(!is_contract_discovery("engine::function::info"));
    }
}
