use std::collections::BTreeSet;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use super::CriterionAward;
use crate::context::E2eContext;
use iii_sdk::protocol::CallRequest;
use iii_sdk::IIIClient;

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
    if function_id == "agent_call" || function_id == "agent_trigger" {
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

/// A `harness::observe` with no target (or `harness::send`) wakes the
/// session; its `function_id` is the observed source. An
/// `engine::register_trigger` (its `trigger_type` says so, before 0.26) named
/// its target `function_id`.
pub fn is_wake_registration(arguments: &Value) -> bool {
    let legacy_target = arguments.get("trigger_type").is_some()
        && arguments.get("function_id").is_some_and(|id| !id.is_null());
    !legacy_target
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
        // `trigger_fired` is the record's name before 0.26.
        .filter(|custom| {
            matches!(
                custom.get("custom_type").and_then(Value::as_str),
                Some("observer_fired" | "trigger_fired")
            )
        })
        .filter_map(|custom| custom.get("data"))
        .collect()
}

pub async fn active_binding_count(context: &E2eContext, session_id: &str) -> anyhow::Result<usize> {
    Ok(context
        .trigger_value(
            "harness::observers::list",
            json!({ "session_id": session_id }),
        )
        .await?
        .get("observers")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(usize::MAX))
}

/// Score a fixture-driven scenario from its atomic observations. `metrics`
/// is the catalog (`id`, `weight`, `measurement`); `validation.observations`
/// holds one `measured` item per metric with `value` (binary) or
/// `numerator`/`denominator` (ratio, `value` deciding a zero denominator).
/// Empty ratios explicitly marked `not_applicable` remain unassessed.
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
            if item["status"] == "not_applicable"
                && metric["measurement"] == "ratio"
                && item["numerator"] == 0
                && item["denominator"] == 0
                && item.get("value").is_none()
            {
                return Ok(CriterionAward {
                    id: metric["id"].as_str().unwrap().into(),
                    awarded: None,
                    reason: item.to_string(),
                });
            }
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

/// SIGTERM every process whose working directory is inside the workspace and
/// return their pids. Reads `/proc`, so elsewhere than Linux nothing is stopped.
pub async fn kill_processes_under(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let pids = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|pid| pid.bytes().all(|byte| byte.is_ascii_digit()))
        .filter(|pid| {
            std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd.starts_with(root))
        })
        .collect::<Vec<_>>();
    if !pids.is_empty() {
        let _ = tokio::process::Command::new("kill")
            .arg("-TERM")
            .args(&pids)
            .stdin(std::process::Stdio::null())
            .output()
            .await;
    }
    pids
}

/// PNG screenshots above this size are recorded by size only.
pub const MAX_SCREENSHOT_BYTES: usize = 4 * 1024 * 1024;

pub fn normalized_block_call(block: &Value) -> Option<(&str, &Value)> {
    if block.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let function = block.get("function_id")?.as_str()?;
    let arguments = block.get("arguments")?;
    // `agent_trigger` is the wrapper's name before 0.26.
    if function == "agent_call" || function == "agent_trigger" {
        return Some((
            arguments.get("function")?.as_str()?,
            arguments.get("payload")?,
        ));
    }
    Some((function, arguments))
}
/// The most `harness::spawn` calls in one assistant response.
pub fn max_parallel_spawns(transcript: &Value) -> usize {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|block| {
                    normalized_block_call(block)
                        .is_some_and(|(function, _)| function == "harness::spawn")
                })
                .count()
        })
        .max()
        .unwrap_or(0)
}
/// The longest run of `harness::spawn` calls with no other call, other
/// function result or new message between them. A run may span responses,
/// since the Harness prompt asks for one large call per response and a
/// spawn's task is large.
pub fn back_to_back_spawns(transcript: &Value) -> usize {
    let (mut run, mut longest) = (0, 0);
    let messages = transcript.get("messages").and_then(Value::as_array);
    for message in messages
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
    {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                let blocks = message.get("content").and_then(Value::as_array);
                // Any call, even one that does not normalize, ends the run.
                for block in blocks.into_iter().flatten().filter(|block| {
                    block.get("type").and_then(Value::as_str) == Some("function_call")
                }) {
                    run = if normalized_block_call(block)
                        .is_some_and(|(function, _)| function == "harness::spawn")
                    {
                        run + 1
                    } else {
                        0
                    };
                    longest = longest.max(run);
                }
            }
            Some("function_result")
                if message.get("function_id").and_then(Value::as_str) == Some("harness::spawn") => {
            }
            _ => run = 0,
        }
    }
    longest
}
pub async fn get_state(context: &E2eContext, scope: &str, key: &str) -> anyhow::Result<Value> {
    Ok(state_value(
        context
            .trigger_value("state::get", json!({ "scope": scope, "key": key }))
            .await?,
    ))
}
pub fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
pub fn collect_files(root: &Path) -> Result<Vec<String>> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(directory)
            .with_context(|| format!("failed reading {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
                paths.push(format!("{relative}#symlink"));
            } else if metadata.is_dir() {
                visit(root, &path, paths)?;
            } else if metadata.is_file() {
                paths.push(path.strip_prefix(root)?.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(root, root, &mut paths)?;
    paths.sort();
    Ok(paths)
}
pub fn remove_directory(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}
pub fn is_remote_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Remote { .. })
    )
}
pub async fn invoke(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    client
        .call(CallRequest {
            function_id: function_id.into(),
            payload,
            action: None,
            timeout_ms: Some(30_000),
        })
        .await
        .map_err(anyhow::Error::new)
        .with_context(|| format!("invoke {function_id}"))
}

/// Text the runner puts in the error of a candidate call it charges as silent.
pub const UNANSWERED: &str = "did not answer";

pub fn is_unanswered(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Timeout)
    )
}

/// Settles one call to a function the candidate owns. The outer error is
/// infrastructure; the inner result is the candidate's answer or its silence.
///
/// The SDK also reports a connection that dropped during the call as a
/// timeout, so a silent call is the candidate's only when the same call, sent
/// once more, is silent again and the stack answers after each silence.
///
/// Nothing cancels the first attempt, so the second can run after or beside
/// it: send through here only a call the candidate's contract makes safe to
/// repeat.
// shortcut: every silent call is confirmed with a second attempt, add a per-capture flag if capture time starts to matter.
pub async fn settle_candidate_call<Call, Sent, Alive, Checked, T>(
    function_id: &str,
    mut call: Call,
    mut stack_alive: Alive,
) -> Result<Result<Value>>
where
    Call: FnMut() -> Sent,
    Sent: Future<Output = Result<Value>>,
    Alive: FnMut() -> Checked,
    Checked: Future<Output = Result<T>>,
{
    let mut result = call().await;
    if result.as_ref().is_err_and(is_unanswered) {
        if let Err(error) = stack_alive().await {
            bail!("invoke {function_id} infrastructure failure: the stack stopped answering after the call timed out: {error:#}");
        }
        result = call().await;
        if result.as_ref().is_err_and(is_unanswered) {
            if let Err(error) = stack_alive().await {
                bail!("invoke {function_id} infrastructure failure: the stack stopped answering after the call timed out again: {error:#}");
            }
            return Ok(result.context(format!(
                "{function_id} {UNANSWERED} twice while the stack still answered"
            )));
        }
    }
    match result {
        Err(error) if !is_remote_failure(&error) => {
            bail!("invoke {function_id} infrastructure failure: {error:#}")
        }
        answer => Ok(answer),
    }
}

pub async fn invoke_candidate(
    context: &E2eContext,
    function_id: &str,
    payload: Value,
) -> Result<Result<Value>> {
    settle_candidate_call(
        function_id,
        || invoke(context.client(), function_id, payload.clone()),
        || context.function_exists(function_id),
    )
    .await
}

/// Appends the silence of a candidate call to the reason of the failed check
/// that observed it. Expected rejections are remote errors and never match.
pub fn name_unanswered_calls(checks: &mut serde_json::Map<String, Value>) {
    fn unanswered(value: &Value) -> Option<&str> {
        match value {
            Value::Object(fields) => fields
                .get("error")
                .and_then(Value::as_str)
                .filter(|error| error.contains(UNANSWERED))
                .or_else(|| fields.values().find_map(unanswered)),
            Value::Array(items) => items.iter().find_map(unanswered),
            _ => None,
        }
    }
    for check in checks
        .values_mut()
        .filter(|check| check.is_object() && check["passed"] != true)
    {
        if let Some(error) = unanswered(check).map(str::to_owned) {
            check["reason"] = match check["reason"].as_str() {
                Some(reason) => format!("{reason}; {error}").into(),
                None => error.into(),
            };
        }
    }
}

pub fn bounded_value(value: Value) -> Value {
    let encoded = value.to_string();
    if encoded.len() <= 16 * 1024 {
        value
    } else {
        json!({"omitted":"response exceeded 16 KiB","sha256":crate::artifact::sha256_bytes(encoded.as_bytes()),"size_bytes":encoded.len()})
    }
}
pub async fn screenshot_png(context: &E2eContext, session: &str) -> Result<Value> {
    let value = context
        .trigger_value(
            "browser::screenshot",
            json!({"session_id":session,"full_page":true,"format":"png"}),
        )
        .await?;
    if value["details"]["session_id"] != session {
        bail!("browser screenshot session identity mismatch");
    }
    let block = value["content"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"] == "image" && item["mime"] == "image/png")
        })
        .context("browser screenshot omitted PNG")?;
    let data = block["data"].as_str().context("browser PNG data missing")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    if bytes.len() > MAX_SCREENSHOT_BYTES {
        return Ok(
            json!({"oversized":true,"size_bytes":bytes.len(),"maximum_bytes":MAX_SCREENSHOT_BYTES,"details":value["details"]}),
        );
    }
    Ok(
        json!({"data":data,"sha256":crate::artifact::sha256_bytes(&bytes),"details":value["details"]}),
    )
}

/// The failed browser check for a step the loaded candidate page rejected.
/// It carries no `status`, so it scores as failed, not blocked.
pub fn page_step_failure(error: &anyhow::Error, url: &str) -> Option<Value> {
    let Some(iii_sdk::errors::Error::Remote { message, .. }) = error.downcast_ref() else {
        return None;
    };
    // shortcut: a hung page gets the browser's answer for a crashed or frozen renderer (its CDP "Request timed out", or none: an SDK timeout), so it stays infrastructure; charge it once the browser reports whether the renderer still answers.
    // Only at the end: a page can echo that text inside its own error, never after the browser's suffix.
    if message.ends_with("Request timed out.") {
        return None;
    }
    let text = format!("{error:#}");
    Some(
        json!({"passed":false,"reason":format!("the candidate page failed a browser step while the browser passed its own check: {text}"),"captures":[],"url":url}),
    )
}

/// Settles the page phase of a browser capture: a failed page step is the
/// candidate's only when the browser still held the session and then passes
/// its own check.
pub async fn settle_page_capture(
    result: Result<Value>,
    session_was_running: bool,
    url: &str,
    browser_healthy: impl Future<Output = Result<()>>,
) -> Result<Value> {
    let error = match result {
        Ok(captured) => return Ok(captured),
        Err(error) => error,
    };
    // A session the browser closed under the step (its Chromium died or the tab's deadline passed) fails that step whatever page was in it.
    let Some(failure) = page_step_failure(&error, url).filter(|_| session_was_running) else {
        return Err(error);
    };
    match browser_healthy.await {
        Ok(()) => Ok(failure),
        Err(unhealthy) => Err(error.context(format!(
            "the browser failed its own check after a page step failed: {unhealthy:#}"
        ))),
    }
}

/// Runs the browser's own command paths in a fresh session. The session stays
/// on its blank page, so no candidate code runs in it. `page_commands` are the
/// further commands the caller's page phase sends; each gets the session's id.
pub async fn browser_answers(context: &E2eContext, page_commands: &[(&str, Value)]) -> Result<()> {
    let started = context
        .trigger_value(
            "browser::sessions::start",
            json!({"incognito":true,"ttl_ms":60000}),
        )
        .await?;
    let session = started["session_id"]
        .as_str()
        .context("browser session omitted session_id")?;
    let checked = async {
        context
            .trigger_value(
                "browser::resize",
                json!({"session_id":session,"width":1280,"height":900}),
            )
            .await?;
        let answer = context
            .trigger_value(
                "browser::execute",
                json!({"session_id":session,"code":"return 1"}),
            )
            .await?;
        if answer["result"] != 1 {
            bail!("browser check script answered {answer}");
        }
        for (function, payload) in page_commands {
            let mut payload = payload.clone();
            payload["session_id"] = session.into();
            context.trigger_value(function, payload).await?;
        }
        screenshot_png(context, session).await?;
        Ok(())
    }
    .await;
    context
        .trigger_value("browser::sessions::stop", json!({"session_id":session}))
        .await
        .context("stop browser check session")?;
    checked
}
pub fn sql_statements(arguments: &Value) -> Vec<&str> {
    arguments
        .get("sql")
        .and_then(Value::as_str)
        .into_iter()
        .chain(
            arguments
                .get("statements")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|statement| {
                    statement
                        .as_str()
                        .or_else(|| statement.get("sql").and_then(Value::as_str))
                }),
        )
        .collect()
}
pub async fn available_databases(context: &E2eContext) -> anyhow::Result<BTreeSet<String>> {
    Ok(context
        .trigger_value("database::listDatabases", json!({}))
        .await?
        .get("databases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|database| {
            database
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect())
}
pub fn sql_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stops_processes_inside_the_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let mut child = tokio::process::Command::new("sleep")
            .arg("60")
            .current_dir(temporary.path())
            .spawn()
            .unwrap();
        let stopped = kill_processes_under(temporary.path()).await;
        assert_eq!(stopped, [child.id().unwrap().to_string()]);
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("the workspace process stopped")
            .unwrap();
        assert!(!status.success());
    }

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

    fn call(id: &str, function: &str) -> Value {
        json!({"type": "function_call", "id": id, "function_id": "agent_trigger",
               "arguments": {"function": function, "payload": {}}})
    }

    fn response(calls: &[(&str, &str)]) -> Value {
        let content: Vec<Value> = calls
            .iter()
            .map(|(id, function)| call(id, function))
            .collect();
        json!({"message": {"role": "assistant", "content": content}})
    }

    fn result(id: &str, function: &str) -> Value {
        json!({"message": {"role": "function_result", "function_call_id": id, "function_id": function}})
    }

    #[test]
    fn parallel_spawns_count_one_response_only() {
        let spread: Vec<Value> = (0..16)
            .flat_map(|index| {
                let id = format!("s{index}");
                [
                    response(&[(&id, "harness::spawn")]),
                    result(&id, "harness::spawn"),
                ]
            })
            .collect();
        let spread = json!({"messages": spread});
        assert_eq!(max_parallel_spawns(&spread), 1);
        assert_eq!(back_to_back_spawns(&spread), 16);

        let ids: Vec<String> = (0..16).map(|index| format!("s{index}")).collect();
        let calls: Vec<(&str, &str)> = ids
            .iter()
            .map(|id| (id.as_str(), "harness::spawn"))
            .collect();
        let one_response = json!({"messages": [response(&calls)]});
        assert_eq!(max_parallel_spawns(&one_response), 16);
    }

    #[test]
    fn a_call_that_does_not_normalize_breaks_the_run() {
        let messages = json!({"messages": [
            response(&[("a", "harness::spawn")]),
            {"message": {"role": "assistant", "content": [
                {"type": "function_call", "id": "x", "function_id": "state::get"}
            ]}},
            response(&[("b", "harness::spawn")]),
        ]});
        assert_eq!(back_to_back_spawns(&messages), 1);
    }

    #[test]
    fn back_to_back_spawn_responses_are_one_fan_out() {
        // The Harness prompt allows one large call per response, so a model
        // spawns three writers in three responses with only their results between.
        let split = json!({"messages": [
            response(&[("r", "engine::register_trigger")]),
            result("r", "engine::register_trigger"),
            response(&[("a", "harness::spawn")]),
            result("a", "harness::spawn"),
            {"custom": {"kind": "trigger_fired"}},
            response(&[("b", "harness::spawn")]),
            result("b", "harness::spawn"),
            response(&[("c", "harness::spawn")]),
            result("c", "harness::spawn"),
        ]});
        assert_eq!(back_to_back_spawns(&split), 3);

        let batched = json!({"messages": [
            response(&[("r", "engine::register_trigger"), ("a", "harness::spawn"), ("b", "harness::spawn")]),
        ]});
        assert_eq!(back_to_back_spawns(&batched), 2);
    }

    #[test]
    fn a_spawn_after_another_call_does_not_join_the_fan_out() {
        let interrupted = json!({"messages": [
            response(&[("a", "harness::spawn")]),
            result("a", "harness::spawn"),
            response(&[("q", "state::get")]),
            result("q", "state::get"),
            response(&[("b", "harness::spawn")]),
            result("b", "harness::spawn"),
            response(&[("c", "harness::spawn")]),
        ]});
        assert_eq!(back_to_back_spawns(&interrupted), 2);

        let mixed = json!({"messages": [
            response(&[("a", "harness::spawn"), ("q", "state::get")]),
            result("a", "harness::spawn"),
            result("q", "state::get"),
            response(&[("b", "harness::spawn")]),
        ]});
        assert_eq!(back_to_back_spawns(&mixed), 1);

        let next_turn = json!({"messages": [
            response(&[("a", "harness::spawn")]),
            result("a", "harness::spawn"),
            {"message": {"role": "user", "content": [{"type": "text", "text": "next"}]}},
            response(&[("b", "harness::spawn")]),
        ]});
        assert_eq!(back_to_back_spawns(&next_turn), 1);
    }

    const CANDIDATE_FUNCTION: &str = "chess_ab12::health";

    fn sdk(error: iii_sdk::errors::Error) -> anyhow::Error {
        anyhow::Error::new(error).context("sent")
    }

    fn silent() -> anyhow::Error {
        sdk(iii_sdk::errors::Error::Timeout)
    }

    fn rejected() -> anyhow::Error {
        sdk(iii_sdk::errors::Error::Remote {
            code: "invocation_failed".into(),
            message: "illegal move".into(),
            stacktrace: None,
        })
    }

    /// Settles one candidate call over scripted answers and returns the
    /// outcome, the calls sent and the stack checks run.
    async fn settle(
        answers: Vec<Result<Value>>,
        stack: Vec<Result<()>>,
    ) -> (Result<Result<Value>>, usize, usize) {
        let mut answers = answers.into_iter();
        let mut stack = stack.into_iter();
        let (mut calls, mut stack_checks) = (0, 0);
        let outcome = settle_candidate_call(
            CANDIDATE_FUNCTION,
            || {
                calls += 1;
                let answer = answers.next().expect("no further call is sent");
                async move { answer }
            },
            || {
                stack_checks += 1;
                let checked = stack.next().expect("no further stack check is run");
                async move { checked }
            },
        )
        .await;
        (outcome, calls, stack_checks)
    }

    #[tokio::test]
    async fn a_silent_candidate_call_is_charged_only_when_the_stack_answers_and_it_stays_silent() {
        let (outcome, calls, stack_checks) = settle(vec![Ok(json!({"ok":true}))], vec![]).await;
        assert_eq!(outcome.unwrap().unwrap(), json!({"ok":true}));
        assert_eq!((calls, stack_checks), (1, 0));

        let (outcome, calls, stack_checks) = settle(vec![Err(rejected())], vec![]).await;
        let rejection = outcome.unwrap().unwrap_err();
        assert!(is_remote_failure(&rejection) && !is_unanswered(&rejection));
        assert_eq!((calls, stack_checks), (1, 0));

        let (outcome, calls, stack_checks) =
            settle(vec![Err(silent()), Err(silent())], vec![Ok(()), Ok(())]).await;
        let silence = outcome.unwrap().unwrap_err();
        let text = format!("{silence:#}");
        assert!(
            text.contains(CANDIDATE_FUNCTION) && text.contains(UNANSWERED),
            "{text}"
        );
        assert!(is_unanswered(&silence) && !is_remote_failure(&silence));
        assert_eq!((calls, stack_checks), (2, 2));

        // The stack went down while the second attempt waited.
        let (outcome, calls, stack_checks) = settle(
            vec![Err(silent()), Err(silent())],
            vec![Ok(()), Err(anyhow::anyhow!("engine gone"))],
        )
        .await;
        let text = format!("{:#}", outcome.unwrap_err());
        assert!(
            text.contains("infrastructure failure") && text.contains(CANDIDATE_FUNCTION),
            "{text}"
        );
        assert_eq!((calls, stack_checks), (2, 2));

        let (outcome, calls, stack_checks) =
            settle(vec![Err(silent()), Ok(json!("late"))], vec![Ok(())]).await;
        assert_eq!(outcome.unwrap().unwrap(), json!("late"));
        assert_eq!((calls, stack_checks), (2, 1));

        let (outcome, calls, stack_checks) =
            settle(vec![Err(silent()), Err(rejected())], vec![Ok(())]).await;
        assert!(is_remote_failure(&outcome.unwrap().unwrap_err()));
        assert_eq!((calls, stack_checks), (2, 1));

        let (outcome, calls, stack_checks) = settle(
            vec![Err(silent())],
            vec![Err(anyhow::anyhow!("engine gone"))],
        )
        .await;
        let text = format!("{:#}", outcome.unwrap_err());
        assert!(
            text.contains("infrastructure failure") && text.contains(CANDIDATE_FUNCTION),
            "{text}"
        );
        assert_eq!((calls, stack_checks), (1, 1));

        let (outcome, calls, stack_checks) =
            settle(vec![Err(sdk(iii_sdk::errors::Error::NotConnected))], vec![]).await;
        assert!(format!("{:#}", outcome.unwrap_err()).contains("infrastructure failure"));
        assert_eq!((calls, stack_checks), (1, 0));

        let (outcome, calls, stack_checks) = settle(
            vec![
                Err(silent()),
                Err(sdk(iii_sdk::errors::Error::NotConnected)),
            ],
            vec![Ok(())],
        )
        .await;
        assert!(format!("{:#}", outcome.unwrap_err()).contains("infrastructure failure"));
        assert_eq!((calls, stack_checks), (2, 1));
    }

    #[tokio::test]
    #[ignore = "requires iii on PATH; CI runs this explicitly with the pinned Engine"]
    async fn a_real_engine_tells_a_hung_candidate_function_from_its_own_death() {
        use std::time::Duration;
        const HUNG: &str = "harness_e2e_test::hung";
        let temp = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let config = temp.path().join("engine.yaml");
        fs::write(&config, format!("workers:\n  - name: iii-worker-manager\n    config:\n      host: 127.0.0.1\n      port: {port}\n")).unwrap();
        let mut engine = tokio::process::Command::new("iii")
            .arg("--no-update-check")
            .arg("--config")
            .arg(&config)
            .current_dir(temp.path())
            .env("III_TELEMETRY_ENABLED", "false")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let context = E2eContext::connect(&format!("ws://127.0.0.1:{port}"))
            .await
            .expect("isolated engine ready");
        // Without local_dispatch(false) the SDK runs a call to this client's own function in-process, and nothing here would cross the engine.
        let hung = context.client().register_function(
            HUNG,
            iii_sdk::RegisterFunction::new_async(|_: Value| {
                std::future::pending::<Result<Value, iii_sdk::errors::Error>>()
            })
            .local_dispatch(false),
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while !context.function_exists(HUNG).await.unwrap() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            !hung.local_dispatch_active(),
            "the hung call must cross the engine"
        );
        // invoke_candidate with a 3 s timeout in place of its 30 s.
        let call = || async {
            let request = CallRequest {
                function_id: HUNG.into(),
                payload: json!({}),
                action: None,
                timeout_ms: Some(3_000),
            };
            Ok(context.client().call(request).await?)
        };
        let stack_alive = || context.function_exists(HUNG);

        let silence = settle_candidate_call(HUNG, call, stack_alive)
            .await
            .unwrap()
            .unwrap_err();
        assert!(is_unanswered(&silence), "{silence:#}");
        assert!(format!("{silence:#}").contains(UNANSWERED), "{silence:#}");

        let (dead, killed) = tokio::join!(settle_candidate_call(HUNG, call, stack_alive), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            engine.kill().await
        });
        killed.unwrap();
        let text = format!("{:#}", dead.unwrap_err());
        assert!(text.contains("infrastructure failure"), "{text}");
    }

    #[test]
    fn chess_and_visual_call_candidate_functions_only_through_invoke_candidate() {
        for source in [
            include_str!("chess_engine_build.rs"),
            include_str!("visual_worker_build.rs"),
        ] {
            let flat: String = source.split_whitespace().collect();
            for (at, _) in flat.match_indices("common::invoke(") {
                // A plain invoke must name a stack function by literal.
                assert!(
                    flat[at..].starts_with("common::invoke(context.client(),\""),
                    "{}",
                    &flat[at..at + 80]
                );
            }
        }
    }

    #[test]
    fn only_unanswered_candidate_calls_are_named_in_a_failed_check() {
        let silence = format!(
            "{CANDIDATE_FUNCTION} {UNANSWERED} twice while the stack still answered: sent: invocation timed out"
        );
        let rejection = "sent: remote error (invocation_failed): illegal move";
        let mut checks = serde_json::Map::new();
        checks.insert(
            "silent".into(),
            json!({"passed":false,"reason":"play contract failed","observed":[{"ok":false,"error":rejection},{"ok":false,"error":silence}]}),
        );
        checks.insert(
            "rejected".into(),
            json!({"passed":false,"reason":"play contract failed","observed":{"ok":false,"error":rejection}}),
        );
        checks.insert(
            "passed".into(),
            json!({"passed":true,"reason":"Worker stayed healthy","observed":{"ok":false,"error":silence}}),
        );
        let before = checks.clone();

        name_unanswered_calls(&mut checks);

        assert_eq!(
            checks["silent"]["reason"],
            format!("play contract failed; {silence}")
        );
        assert_eq!(checks["silent"]["observed"], before["silent"]["observed"]);
        assert_eq!(checks["rejected"], before["rejected"]);
        assert_eq!(checks["passed"], before["passed"]);
    }

    const PAGE_URL: &str = "http://127.0.0.1:3113/#/worker/chess_ab12/chess";

    fn page_error(message: &str) -> anyhow::Error {
        sdk(iii_sdk::errors::Error::Remote {
            code: "invocation_failed".into(),
            message: message.into(),
            stacktrace: None,
        })
    }

    #[test]
    fn only_a_page_step_error_becomes_a_failed_browser_check() {
        let error = page_error("handler error: navigation failed");
        let failure = page_step_failure(&error, PAGE_URL).expect("page step failure");
        assert_eq!(failure["passed"], false);
        assert!(failure.get("status").is_none());
        assert_eq!(failure["captures"], json!([]));
        assert_eq!(failure["url"], PAGE_URL);
        assert!(failure["reason"]
            .as_str()
            .unwrap()
            .ends_with(&format!("{error:#}")));
        // The page serializes its own script envelope, so it can echo the
        // browser's words back; serde's position suffix then ends the text.
        for forged in [
            "handler error: script envelope did not parse: invalid type: string \"unknown session 'b1'; list tabs with browser::sessions::list\", expected struct Envelope at line 1 column 62",
            "handler error: script envelope did not parse: invalid type: string \"screenshot failed: Request timed out.\", expected struct Envelope at line 1 column 39",
        ] {
            assert!(
                page_step_failure(&page_error(forged), PAGE_URL).is_some(),
                "{forged}"
            );
        }
        for error in [
            // A hung page and a crashed renderer both end in one of these.
            page_error("handler error: key down failed: Request timed out."),
            page_error("handler error: resize failed: Request timed out."),
            page_error("handler error: screenshot failed: Request timed out."),
            silent(),
            sdk(iii_sdk::errors::Error::NotConnected),
            anyhow::anyhow!("browser screenshot session identity mismatch"),
            // The Canvas-page steps drop the SDK error type this way.
            anyhow::anyhow!("{:#}", page_error("handler error: navigation failed")),
        ] {
            assert_eq!(page_step_failure(&error, PAGE_URL), None, "{error:#}");
        }
    }

    #[test]
    fn the_browser_check_runs_the_browser_functions_its_scenario_calls_and_no_others() {
        fn functions(text: &str) -> impl Iterator<Item = &str> {
            text.match_indices("\"browser::")
                .map(move |(at, _)| &text[at..at + 2 + text[at + 1..].find('"').unwrap()])
        }
        let common = include_str!("common.rs");
        let shared = &common[common.find("pub async fn browser_answers").unwrap()..];
        let shared = &shared[..shared.find("\npub fn ").unwrap()];
        for source in [
            include_str!("chess_engine_build.rs"),
            include_str!("visual_worker_build.rs"),
        ] {
            let own = &source[source.find("browser_answers(").unwrap()..];
            let own = &own[..own.find("\n}").unwrap()];
            for function in functions(source) {
                // Navigation loads the candidate page; the blank session has none.
                if function != "\"browser::navigate\"" {
                    assert!(
                        shared.contains(function) || own.contains(function),
                        "{function}"
                    );
                }
            }
            // A command the scenario never sends must not decide its page faults.
            for function in functions(shared) {
                assert!(source.contains(function), "{function}");
            }
        }
    }

    #[tokio::test]
    async fn a_page_step_failure_counts_only_when_the_browser_passes_its_own_check() {
        let page_failed = || Err(page_error("handler error: navigation failed"));

        let charged = settle_page_capture(page_failed(), true, PAGE_URL, async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(charged["passed"], false);
        assert!(charged.get("status").is_none());

        let unhealthy = settle_page_capture(page_failed(), true, PAGE_URL, async {
            Err(anyhow::anyhow!("browser::resize timed out"))
        })
        .await
        .unwrap_err();
        let unhealthy = format!("{unhealthy:#}");
        assert!(unhealthy.contains("navigation failed") && unhealthy.contains("resize timed out"));

        let checked = std::cell::Cell::new(false);
        let browser_check = || async {
            checked.set(true);
            Ok(())
        };
        let captured = json!({"passed":true,"captures":["before"]});
        assert_eq!(
            settle_page_capture(Ok(captured.clone()), true, PAGE_URL, browser_check())
                .await
                .unwrap(),
            captured
        );
        let internal = settle_page_capture(
            Err(anyhow::anyhow!("browser screenshot omitted PNG")),
            true,
            PAGE_URL,
            browser_check(),
        )
        .await
        .unwrap_err();
        assert_eq!(format!("{internal:#}"), "browser screenshot omitted PNG");
        // The browser closed the session under the step (Chromium died) or
        // lost its record of it.
        for message in [
            "handler error: screenshot failed: oneshot canceled",
            "handler error: unknown session 'b1'; list tabs with browser::sessions::list",
        ] {
            let closed =
                settle_page_capture(Err(page_error(message)), false, PAGE_URL, browser_check())
                    .await
                    .unwrap_err();
            assert!(format!("{closed:#}").contains(message));
        }
        assert!(!checked.get());
    }
}
