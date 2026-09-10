//! The Linkly agentic tutorial as one scripted-dialogue scenario.
//!
//! Eight exchanges on one Harness session — the seven tutorial chapters plus
//! the project-wide `compose::restart` guard — against the `linkly-agentic`
//! scaffold's own Compose stack. The Harness scopes every `compose::*` call to
//! the project file it was started from, so the scaffold *is* the stack under
//! test: the runner attaches to it (`run --url`), `setup` preflights it and
//! samples the session while the dialogue runs, and `capture` validates the
//! finished project with twenty-two deterministic checks. Nothing here creates or
//! tears down the stack; `scripts/linkly_stack.py` does that.
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde_json::{json, Value};
use tokio::process::Command;

use super::common::{atomic_awards, evidence_bundle, function_outcomes, state_value};
use super::*;
use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

pub const ID: &str = "linkly_tutorial";
pub const VERSION: u32 = 1;
const EVIDENCE_ID: &str = "linkly_evidence";
const HTTP_BASE: &str = "http://127.0.0.1:3111";
const PROXY_ADDR: &str = "127.0.0.1:3110";
const TEMPLATE_SOURCE: &str = "linkly-agentic";
/// Containers the scaffold starts before Ch. 1; the agent adds the rest.
const BASELINE_CONTAINERS: [&str; 9] = [
    "http",
    "state",
    "cron",
    "queue",
    "shell",
    "harness",
    "llm-router",
    "session-manager",
    "iii-directory",
];
/// Directories the agent creates from Ch. 4 on; a fresh scaffold has none.
const AGENT_DIRECTORIES: [&str; 6] = [
    "analytics",
    "click-streamer",
    "bulk-importer",
    "channel-client",
    "auth",
    "frontend",
];
const CHAPTERS: [&str; 8] = [
    "ch1_foundations",
    "ch2_observe",
    "ch3_persist",
    "ch4_durable",
    "ch5_stream",
    "ch6_channels",
    "ch7_browser",
    "guard",
];
const PROMPTS: [&str; 8] = [
    include_str!("../../tests/fixtures/linkly-tutorial/ch-1-foundations.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/ch-2-observe.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/ch-3-persist.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/ch-4-durable.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/ch-5-stream.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/ch-6-channels.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/ch-7-browser.md"),
    include_str!("../../tests/fixtures/linkly-tutorial/guard.md"),
];
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EXCERPT_BYTES: usize = 8 * 1024;

fn metrics() -> &'static [Value] {
    static ALL: OnceLock<Vec<Value>> = OnceLock::new();
    ALL.get_or_init(|| {
        let catalog: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/linkly-tutorial/metrics.json"
        ))
        .expect("Linkly metrics JSON");
        catalog["tests"]
            .as_array()
            .expect("Linkly metrics tests")
            .iter()
            .flat_map(|test| {
                test["metrics"]
                    .as_array()
                    .expect("Linkly metrics array")
                    .clone()
            })
            .collect()
    })
}
fn digest(run_id: &str) -> String {
    crate::artifact::sha256_bytes(run_id.as_bytes()).replace("sha256:", "")
}
fn root(run_id: &str) -> PathBuf {
    std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("harness-e2e-linkly")
        .join(format!("{ID}-{}", digest(run_id)))
}
/// The runner names the subject session after the attempt.
fn session_id(run_id: &str) -> String {
    format!("e2e_{run_id}")
}
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn scenario(_run_id: &str) -> ScenarioSpec {
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: PROMPTS[0].trim_end().to_string(),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 256,
            max_output_tokens: Some(32_768),
            max_total_tokens: Some(6_000_000),
            stuck_timeout_seconds: 900,
            max_validation_retries: None,
        },
        denied_functions: &[
            "approval::*",
            "configuration::register",
            "shell::workspace::*",
        ],
        criteria: metrics()
            .iter()
            .map(|m| {
                CriterionSpec::scored(
                    m["id"].as_str().unwrap(),
                    m["weight"].as_u64().unwrap() as u8,
                    m["question"].as_str().unwrap(),
                    EvaluationDimension::Deliverable,
                )
            })
            .collect(),
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

/// Chapters 2–7 and the guard, sent on the same session after the first
/// prompt completes.
pub fn dialogue_followups(_run_id: &str) -> Vec<String> {
    PROMPTS[1..]
        .iter()
        .map(|prompt| prompt.trim_end().to_string())
        .collect()
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case: ScenarioCase::new(
            ID,
            VERSION,
            super::stable_seed(ID),
            json!({
                "template": TEMPLATE_SOURCE,
                "chapters": 7,
                "guard": "compose::restart without a container is refused",
                "exchanges": PROMPTS.len(),
                "prompts_sha256": crate::artifact::sha256_bytes(PROMPTS.concat().as_bytes()),
            }),
            ComplexityProfile {
                planning_depth: 7,
                dependency_depth: 4,
                parallel_branches: 3,
                external_systems: 4,
                state_transitions: 12,
                validation_loops: 3,
                artifact_count: 1,
                coordination_edges: 6,
                ambiguity_level: 3,
                ..Default::default()
            },
            vec![
                "e2e::control-plane-v1".into(),
                "iii::functions".into(),
                "iii::compose".into(),
                "harness::scripted-dialogue-v1".into(),
                "node".into(),
                "curl".into(),
            ],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: EVIDENCE_ID.into(),
                    kind: "application_audit".into(),
                    media_type: "application/json".into(),
                    schema: json!({"type":"object","required":["observations","root","files","chapters"]}),
                    max_size_bytes: crate::asset::DEFAULT_MAX_CAPTURE_BYTES,
                }],
                invariants: vec![],
                provenance_required: true,
                capture_before_cleanup: true,
            },
        )?,
        capture: Some(capture),
    })
}

/// The subject's `fs_scope` root once `setup` has found the project; `None`
/// for other scenarios or before setup ran.
pub fn prepared_filesystem_root(scenario_id: &str, run_id: &str) -> Result<Option<PathBuf>> {
    if scenario_id != ID {
        return Ok(None);
    }
    let path = root(run_id).join("project.json");
    if !path.is_file() {
        return Ok(None);
    }
    let project: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    Ok(project["project_root"].as_str().map(PathBuf::from))
}

/// Why `project` is not a fresh `linkly-agentic` scaffold; empty when it is.
fn scaffold_problems(project: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    match std::fs::read_to_string(project.join(".iii/project.ini")) {
        Ok(ini) => {
            if !ini
                .lines()
                .any(|line| line.trim() == format!("source={TEMPLATE_SOURCE}"))
            {
                problems.push(format!(
                    ".iii/project.ini does not name source={TEMPLATE_SOURCE}"
                ));
            }
        }
        Err(_) => problems.push(".iii/project.ini is missing".into()),
    }
    for required in ["worker-compose.yaml", "link/src/index.ts"] {
        if !project.join(required).is_file() {
            problems.push(format!("{required} is missing"));
        }
    }
    for directory in AGENT_DIRECTORIES {
        if project.join(directory).is_dir() {
            problems.push(format!("{directory}/ already exists"));
        }
    }
    problems
}
fn ready_containers(status: &Value) -> Vec<String> {
    status["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|container| container["state"] == "ready")
        .filter_map(|container| container["container"].as_str().map(str::to_owned))
        .collect()
}
fn function_ids(listed: &Value) -> Vec<String> {
    listed["functions"]
        .as_array()
        .or_else(|| listed.as_array())
        .into_iter()
        .flatten()
        .filter_map(|function| function["function_id"].as_str().map(str::to_owned))
        .collect()
}

struct Monitor {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}
fn monitors() -> &'static Mutex<HashMap<String, Monitor>> {
    static VALUE: OnceLock<Mutex<HashMap<String, Monitor>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashMap::new()))
}
async fn trigger(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    client
        .trigger(TriggerRequest {
            function_id: function_id.to_string(),
            payload,
            action: None,
            timeout_ms: Some(10_000),
        })
        .await
        .map_err(|error| anyhow!("{function_id}: {error}"))
}
/// Sample the subject session every few seconds while the dialogue runs:
/// root turn id, status and step plus the tree's usage totals. The runner
/// only exposes the whole dialogue's metrics, so this is where per-chapter
/// timing and spend come from (`chapters()`).
fn spawn_monitor(context: &E2eContext, run_id: &str, samples: PathBuf) {
    let client = context.client().clone();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let session = session_id(run_id);
    let handle = tokio::spawn(async move {
        let started = Instant::now();
        let mut last_key = None;
        while !flag.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(6 * 3600) {
            let status = trigger(
                &client,
                "harness::status",
                json!({"session_id": session, "verbose": false}),
            )
            .await
            .unwrap_or(Value::Null);
            let metrics = trigger(
                &client,
                "harness::metrics",
                json!({"root_session_id": session}),
            )
            .await
            .unwrap_or(Value::Null);
            let sample = json!({
                "at_ms": now_ms(),
                "turn_id": status["turn_id"],
                "status": status["status"],
                "step": status["step"],
                "turn_count": status["turn_count"],
                "totals": tree_totals(&metrics),
            });
            let key = format!(
                "{}|{}|{}|{}|{}",
                sample["turn_id"],
                sample["status"],
                sample["step"],
                sample["turn_count"],
                sample["totals"]
            );
            if last_key.as_deref() != Some(key.as_str()) {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&samples)
                {
                    let _ = writeln!(file, "{sample}");
                }
                last_key = Some(key);
            }
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
    monitors()
        .lock()
        .unwrap()
        .insert(run_id.to_string(), Monitor { stop, handle });
}
/// `harness::metrics` totals plus every numeric per-session usage field
/// (`input_tokens`, `output_tokens`, `reasoning_tokens`, `cache_read_tokens`,
/// `cost_usd`, …) summed over the tree — the totals object alone carries only
/// turn and call counts.
fn tree_totals(metrics: &Value) -> Value {
    let counts = metrics["totals"].as_object().cloned().unwrap_or_default();
    let mut totals = counts.clone();
    for session in metrics["by_session"].as_array().into_iter().flatten() {
        for (key, value) in session.as_object().into_iter().flatten() {
            // Turn and call counts are already summed in `totals`; `depth` is
            // a tree position, not usage.
            if counts.contains_key(key) || key == "depth" {
                continue;
            }
            let Some(number) = value.as_f64() else {
                continue;
            };
            let entry = totals.entry(key.clone()).or_insert_with(|| json!(0.0));
            let current = entry.as_f64().unwrap_or(0.0);
            *entry = json!(current + number);
        }
    }
    if totals.is_empty() {
        Value::Null
    } else {
        Value::Object(totals)
    }
}
fn stop_monitor(run_id: &str) {
    if let Some(monitor) = monitors().lock().unwrap().remove(run_id) {
        monitor.stop.store(true, Ordering::Relaxed);
        monitor.handle.abort();
    }
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let directory = root(run_id);
        std::fs::create_dir_all(directory.join("validation/checks"))?;
        std::fs::create_dir_all(directory.join("validation/http"))?;
        let status = context
            .trigger_value("compose::status", json!({}))
            .await
            .context(
            "compose::status failed; is the Linkly stack up and the runner attached to its engine?",
        )?;
        let compose_file = PathBuf::from(
            status["file"]
                .as_str()
                .context("compose::status did not name the project file")?,
        );
        let discovered = compose_file
            .parent()
            .context("compose file has no parent directory")?
            .to_path_buf();
        let project = match std::env::var_os("HARNESS_E2E_LINKLY_PROJECT") {
            Some(explicit) => {
                let explicit = PathBuf::from(explicit);
                if explicit != discovered {
                    bail!(
                        "HARNESS_E2E_LINKLY_PROJECT ({}) is not the project Compose is running ({})",
                        explicit.display(),
                        discovered.display()
                    );
                }
                explicit
            }
            None => discovered,
        };
        let problems = scaffold_problems(&project);
        if !problems.is_empty() {
            bail!(
                "{} is not a fresh {TEMPLATE_SOURCE} scaffold: {}",
                project.display(),
                problems.join("; ")
            );
        }
        let ready = ready_containers(&status);
        let missing: Vec<_> = BASELINE_CONTAINERS
            .iter()
            .filter(|name| !ready.iter().any(|ready| ready == *name))
            .collect();
        if !missing.is_empty() {
            bail!("baseline containers are not ready: {missing:?} (ready: {ready:?})");
        }
        let providers: Vec<_> = ready
            .iter()
            .filter(|name| name.starts_with("provider-"))
            .cloned()
            .collect();
        if providers.is_empty() {
            bail!("no provider-* container is ready; enable one in worker-compose.yaml with its key in .env");
        }
        let listed = context
            .trigger_value("engine::functions::list", json!({"prefix": "link::"}))
            .await?;
        let existing: Vec<_> = function_ids(&listed)
            .into_iter()
            .filter(|id| id.starts_with("link::"))
            .collect();
        if !existing.is_empty() {
            bail!("link functions are already registered ({existing:?}); this scaffold has been built before");
        }
        let probe = curl(&[
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "10",
            &format!("{HTTP_BASE}/s/e2e-preflight"),
        ])
        .await
        .context("the http worker did not answer on 127.0.0.1:3111")?;
        let record = json!({
            "project_root": project,
            "compose_file": compose_file,
            "namespace": status["namespace"],
            "ready_containers": ready,
            "providers": providers,
            "session_id": session_id(run_id),
            "http_probe": probe.trim(),
            "at_ms": now_ms(),
        });
        std::fs::write(
            directory.join("project.json"),
            serde_json::to_vec_pretty(&record)?,
        )?;
        std::fs::write(
            directory.join("validation/preflight.json"),
            serde_json::to_vec_pretty(&json!({"record": record, "compose_status": status}))?,
        )?;
        spawn_monitor(context, run_id, directory.join("validation/samples.jsonl"));
        Ok(())
    })
}

/// Run `curl` with `args`; stdout on success (non-zero exit is an error).
/// A missing binary surfaces as `io::ErrorKind::NotFound`, which the checks
/// report as unavailable rather than as a product failure.
async fn curl(args: &[&str]) -> Result<String> {
    let output = Command::new("curl")
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| anyhow!(error).context("spawn curl"))?;
    if !output.status.success() {
        bail!(
            "curl exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
fn is_missing_tool(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::NotFound)
}
fn excerpt(value: &Value) -> Value {
    let text = value.to_string();
    if text.len() <= EXCERPT_BYTES {
        return value.clone();
    }
    let mut cut = EXCERPT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    json!({"truncated": true, "bytes": text.len(), "head": &text[..cut]})
}
fn truncate_text(text: &str) -> String {
    if text.len() <= EXCERPT_BYTES {
        return text.to_string();
    }
    let mut cut = EXCERPT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}… [{} bytes]", &text[..cut], text.len())
}
/// Does `value` contain `needle` as a string anywhere in its tree?
fn contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text == needle,
        Value::Array(items) => items.iter().any(|item| contains_string(item, needle)),
        Value::Object(map) => map.values().any(|item| contains_string(item, needle)),
        _ => false,
    }
}
fn contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Array(items) => items.iter().any(|item| contains_key(item, key)),
        Value::Object(map) => {
            map.contains_key(key) || map.values().any(|item| contains_key(item, key))
        }
        _ => false,
    }
}
/// The list a function answered with: a bare array, or the first array-valued
/// field of an object (`traces`, `rows`, `topics`, `functions`, `items`).
fn listed(value: &Value) -> Vec<Value> {
    if let Some(items) = value.as_array() {
        return items.clone();
    }
    for key in [
        "traces",
        "rows",
        "topics",
        "functions",
        "items",
        "data",
        "roots",
    ] {
        if let Some(items) = value[key].as_array() {
            return items.clone();
        }
    }
    value
        .as_object()
        .and_then(|map| map.values().find_map(|item| item.as_array().cloned()))
        .unwrap_or_default()
}
/// Sum of every numeric `count` column over the rows of a query result.
fn sum_counts(rows: &[Value]) -> f64 {
    rows.iter().filter_map(|row| row["count"].as_f64()).sum()
}
/// Parse "imported N … skipped M" out of a client's output (prose or JSON).
fn import_counts(text: &str) -> Option<(u64, u64)> {
    let lower = text.to_ascii_lowercase();
    let number_after = |keyword: &str| {
        let start = lower.rfind(keyword)? + keyword.len();
        let window = &lower[start..lower.len().min(start + 32)];
        let digits: String = window
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    };
    Some((number_after("imported")?, number_after("skipped")?))
}
/// The root turn that answered each runner message, in order. Wake
/// notifications (`origin.notification`, text `[notification] …`) also arrive
/// as user messages and open turns of their own, so an exchange spans every
/// turn from its own first turn up to the next runner message.
fn exchange_first_turns(transcript: &Value) -> Vec<String> {
    let mut turns = Vec::new();
    let mut awaiting_reply = false;
    for entry in transcript["messages"].as_array().into_iter().flatten() {
        let message = &entry["message"];
        match message["role"].as_str() {
            Some("user") => {
                let text = message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<String>();
                let notification = entry["origin"]["notification"] == true
                    || text.trim_start().starts_with("[notification]");
                if !notification {
                    awaiting_reply = true;
                }
            }
            Some("assistant") if awaiting_reply => {
                if let Some(turn_id) = entry["origin"]["turn_id"].as_str() {
                    turns.push(turn_id.to_string());
                    awaiting_reply = false;
                }
            }
            _ => {}
        }
    }
    turns
}
/// Per-exchange timing and spend from the monitor's samples. A chapter opens
/// at the first sample of the turn that answered the runner's message
/// (`first_turns`, from the transcript) and absorbs every later sample —
/// including the short wake turns the 5 s sampler may never see — until the
/// next runner message's turn shows up; `work_seconds` stops at the last
/// sample where the session was not `completed`. Totals deltas subtract the
/// last totals seen before the chapter opened. Without a transcript every
/// distinct root turn is treated as a chapter instead.
fn chapters(samples: &[Value], first_turns: &[String]) -> Vec<Value> {
    let per_turn = first_turns.is_empty();
    let mut chapters: Vec<Value> = Vec::new();
    let mut current: Option<usize> = None;
    let mut previous_totals = Value::Null;
    for sample in samples {
        let Some(turn_id) = sample["turn_id"].as_str() else {
            continue;
        };
        let at = sample["at_ms"].as_u64().unwrap_or(0);
        let opens = if per_turn {
            chapters
                .last()
                .is_none_or(|last| last["turn_id"] != turn_id)
                .then_some(chapters.len())
        } else {
            first_turns
                .iter()
                .position(|first| first == turn_id)
                .filter(|index| current.is_none_or(|current| *index > current))
        };
        if let Some(index) = opens {
            current = Some(index);
            chapters.push(json!({
                "exchange": index,
                "chapter": CHAPTERS.get(index).copied().unwrap_or("extra"),
                "turn_id": turn_id,
                "turn_ids": [turn_id],
                "started_ms": at,
                "ended_ms": at,
                "last_active_ms": at,
                "turn_count": sample["turn_count"],
                "totals_start": previous_totals.clone(),
                "totals_end": sample["totals"],
            }));
        }
        if current.is_none() {
            continue;
        }
        let last = chapters.last_mut().expect("a chapter is open");
        if let Some(ids) = last["turn_ids"].as_array_mut() {
            if !ids.iter().any(|id| id == turn_id) {
                ids.push(json!(turn_id));
            }
        }
        last["ended_ms"] = json!(at);
        last["turn_count"] = sample["turn_count"].clone();
        last["totals_end"] = sample["totals"].clone();
        if sample["status"] != "completed" {
            last["last_active_ms"] = json!(at);
        }
        if !sample["totals"].is_null() {
            previous_totals = sample["totals"].clone();
        }
    }
    for chapter in &mut chapters {
        let started = chapter["started_ms"].as_u64().unwrap_or(0);
        let ended = chapter["ended_ms"].as_u64().unwrap_or(started);
        let active = chapter["last_active_ms"].as_u64().unwrap_or(started);
        chapter["seconds"] = json!((ended.saturating_sub(started)) as f64 / 1000.0);
        chapter["work_seconds"] = json!((active.saturating_sub(started)) as f64 / 1000.0);
        chapter["totals_delta"] = totals_delta(&chapter["totals_start"], &chapter["totals_end"]);
        chapter["derivation"] = json!(if per_turn {
            "per_root_turn"
        } else {
            "runner_message"
        });
    }
    chapters
}
fn totals_delta(start: &Value, end: &Value) -> Value {
    let Some(end) = end.as_object() else {
        return Value::Null;
    };
    let mut delta = serde_json::Map::new();
    for (key, value) in end {
        if let Some(after) = value.as_f64() {
            let before = start[key].as_f64().unwrap_or(0.0);
            delta.insert(key.clone(), json!(after - before));
        }
    }
    Value::Object(delta)
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn project_tree(project: &Path) -> String {
    const SKIP: [&str; 6] = [
        "node_modules",
        "data",
        ".iii",
        "dist",
        ".git",
        ".browser-sdk",
    ];
    fn walk(directory: &Path, depth: usize, prefix: &str, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            if is_dir {
                out.push_str(&format!("{prefix}{name}/\n"));
                if depth < 3 && !SKIP.contains(&name.as_str()) {
                    walk(&entry.path(), depth + 1, &format!("{prefix}  "), out);
                }
            } else {
                out.push_str(&format!("{prefix}{name} ({size})\n"));
            }
        }
    }
    let mut out = String::new();
    walk(project, 0, "", &mut out);
    out
}

type PollFuture<'b, T> = Pin<Box<dyn std::future::Future<Output = Result<Option<T>>> + Send + 'b>>;

struct Http {
    status: u16,
    redirect: String,
    body: String,
}
struct Outcome {
    pass: bool,
    reason: String,
}
fn pass(reason: impl Into<String>) -> Outcome {
    Outcome {
        pass: true,
        reason: reason.into(),
    }
}
fn fail(reason: impl Into<String>) -> Outcome {
    Outcome {
        pass: false,
        reason: reason.into(),
    }
}

/// One validation pass over the finished project: every check appends its
/// evidence steps, then `record` turns them into an observation file.
struct Probe<'a> {
    context: &'a E2eContext,
    validation: PathBuf,
    project: PathBuf,
    tag: String,
    transcript: &'a Value,
    preflight_ready: Vec<String>,
    session: String,
    steps: Vec<Value>,
    http_counter: usize,
    observations: Vec<Value>,
    trace_id: Option<String>,
    stream_items: Vec<Value>,
}
impl Probe<'_> {
    async fn call(&mut self, function_id: &str, payload: Value) -> Result<Value> {
        let started = Instant::now();
        let result = self
            .context
            .trigger_value(function_id, payload.clone())
            .await;
        self.steps.push(json!({
            "kind": "function",
            "function_id": function_id,
            "payload": payload,
            "elapsed_ms": started.elapsed().as_millis(),
            "response": match &result { Ok(value) => excerpt(value), Err(error) => json!({"error": format!("{error:#}")}) },
        }));
        result
    }
    async fn http(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Http> {
        let url = format!("{HTTP_BASE}{path}");
        let out = self
            .validation
            .join(format!("http/{:03}.body", self.http_counter));
        self.http_counter += 1;
        let out_text = out.to_string_lossy().into_owned();
        let body_text = body.as_ref().map(Value::to_string);
        let mut args = vec![
            "-sS",
            "-o",
            &out_text,
            "-w",
            "%{http_code}\n%{redirect_url}",
            "--max-time",
            "20",
            "-X",
            method,
        ];
        if let Some(body) = &body_text {
            args.extend(["-H", "Content-Type: application/json", "-d", body]);
        }
        args.push(&url);
        let started = Instant::now();
        let result = curl(&args).await;
        let response = match &result {
            Ok(stdout) => {
                let mut lines = stdout.lines();
                let status = lines.next().unwrap_or("").trim().parse().unwrap_or(0);
                let redirect = lines.next().unwrap_or("").trim().to_string();
                let body = std::fs::read_to_string(&out).unwrap_or_default();
                Ok(Http {
                    status,
                    redirect,
                    body,
                })
            }
            Err(error) => Err(anyhow!("{error:#}")),
        };
        self.steps.push(json!({
            "kind": "http",
            "method": method,
            "url": url,
            "request": body,
            "elapsed_ms": started.elapsed().as_millis(),
            "status": response.as_ref().map(|r| r.status).ok(),
            "redirect": response.as_ref().map(|r| r.redirect.clone()).ok(),
            "response": response.as_ref().map(|r| truncate_text(&r.body)).unwrap_or_else(|error| error.to_string()),
        }));
        response
    }
    async fn command(
        &mut self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<(i32, String, String)> {
        let started = Instant::now();
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&self.project)
            .kill_on_drop(true);
        let result = tokio::time::timeout(timeout, command.output()).await;
        let outcome = match result {
            Ok(Ok(output)) => Ok((
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )),
            Ok(Err(error)) => Err(anyhow!(error).context(format!("spawn {program}"))),
            Err(_) => Err(anyhow!("{program} exceeded {}s", timeout.as_secs())),
        };
        self.steps.push(json!({
            "kind": "command",
            "program": program,
            "args": args,
            "cwd": self.project,
            "elapsed_ms": started.elapsed().as_millis(),
            "exit_code": outcome.as_ref().map(|o| o.0).ok(),
            "stdout": outcome.as_ref().map(|o| truncate_text(&o.1)).ok(),
            "stderr": outcome.as_ref().map(|o| truncate_text(&o.2)).unwrap_or_else(|error| format!("{error:#}")),
        }));
        outcome
    }
    /// Poll `probe` every two seconds until it yields a value or `timeout`.
    async fn poll<T>(
        &mut self,
        timeout: Duration,
        mut probe: impl for<'b> FnMut(&'b mut Self) -> PollFuture<'b, T>,
    ) -> Result<Option<T>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = probe(self).await? {
                return Ok(Some(found));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    fn record(&mut self, id: &str, outcome: Result<Outcome>) {
        let steps = std::mem::take(&mut self.steps);
        let (status, value, reason) = match outcome {
            Ok(outcome) => ("measured", u8::from(outcome.pass), outcome.reason),
            Err(error) if is_missing_tool(&error) => ("unavailable", 0, format!("{error:#}")),
            Err(error) => ("measured", 0, format!("{error:#}")),
        };
        let observation = json!({"id": id, "status": status, "value": value, "reason": reason});
        let _ = write_json(
            &self.validation.join(format!("checks/{id}.json")),
            &json!({"observation": observation, "steps": steps}),
        );
        self.observations.push(observation);
    }

    async fn run_all(&mut self) {
        let tag = self.tag.clone();
        // Ch. 1 — the four HTTP behaviours around one fresh code.
        let code = format!("e2e-{tag}");
        let url = format!("https://example.org/e2e/{tag}");
        let created = self
            .http("POST", "/links", Some(json!({"url": url, "code": code})))
            .await;
        self.record(
            "foundations.create",
            created.map(|http| {
                let body: Value = serde_json::from_str(&http.body).unwrap_or(Value::Null);
                if http.status == 201 && body["code"] == code {
                    pass(format!("201 with code {code}"))
                } else {
                    fail(format!(
                        "status {} body {}",
                        http.status,
                        truncate_text(&http.body)
                    ))
                }
            }),
        );
        let redirect = self.http("GET", &format!("/s/{code}"), None).await;
        self.record(
            "foundations.redirect",
            redirect.map(|http| {
                if http.status == 302 && http.redirect.trim_end_matches('/') == url {
                    pass(format!("302 to {url}"))
                } else {
                    fail(format!(
                        "status {} location {:?}",
                        http.status, http.redirect
                    ))
                }
            }),
        );
        let conflict = self
            .http("POST", "/links", Some(json!({"url": url, "code": code})))
            .await;
        self.record(
            "foundations.conflict",
            conflict.map(|http| {
                if http.status == 409 {
                    pass("409 on a taken code")
                } else {
                    fail(format!("status {}", http.status))
                }
            }),
        );
        let unknown = self.http("GET", &format!("/s/nope-{tag}"), None).await;
        self.record(
            "foundations.unknown",
            unknown.map(|http| {
                if http.status == 404 {
                    pass("404 on an unknown code")
                } else {
                    fail(format!("status {}", http.status))
                }
            }),
        );

        // Ch. 2 — the redirects above left traces named after the route.
        let traces = self
            .poll(Duration::from_secs(10), |probe| {
                Box::pin(async move {
                    let value = probe
                        .call(
                            "engine::traces::list",
                            json!({"name": "GET /s/:code", "limit": 5}),
                        )
                        .await?;
                    let traces = listed(&value);
                    Ok((!traces.is_empty()).then_some(traces))
                })
            })
            .await;
        let (listed_traces, trace_id) = match traces {
            Ok(Some(traces)) => {
                let trace_id = traces[0]["trace_id"].as_str().map(str::to_owned);
                (
                    Ok(pass(format!(
                        "{} trace(s) named GET /s/:code",
                        traces.len()
                    ))),
                    trace_id,
                )
            }
            Ok(None) => (Ok(fail("no trace named GET /s/:code within 10s")), None),
            Err(error) => (Err(error), None),
        };
        self.trace_id = trace_id;
        self.record("observe.traces_listed", listed_traces);
        let tree = match self.trace_id.clone() {
            Some(trace_id) => self
                .call("engine::traces::tree", json!({"trace_id": trace_id}))
                .await
                .map(|value| {
                    let roots = listed(&value);
                    if roots.is_empty() {
                        fail("tree has no root span")
                    } else {
                        pass(format!("{} root span(s)", roots.len()))
                    }
                }),
            None => Ok(fail("no listed trace to expand")),
        };
        self.record("observe.tree_resolves", tree);

        // Ch. 3 — three links, one redirect each, then the two tables and the cache.
        let persist_codes: Vec<String> =
            (1..=3).map(|n| format!("e2e-persist-{tag}-{n}")).collect();
        let persist_url = |n: usize| format!("https://example.org/e2e/{tag}/persist/{n}");
        let mut created_all = true;
        for (n, code) in persist_codes.iter().enumerate() {
            match self
                .http(
                    "POST",
                    "/links",
                    Some(json!({"url": persist_url(n + 1), "code": code})),
                )
                .await
            {
                Ok(http) if http.status == 201 => {}
                _ => created_all = false,
            }
        }
        let codes = persist_codes.clone();
        let rows = self
            .poll(Duration::from_secs(10), |probe| {
                let codes = codes.clone();
                Box::pin(async move {
                    let value = probe
                        .call(
                            "database::query",
                            json!({"db": "primary", "sql": "SELECT * FROM links"}),
                        )
                        .await?;
                    let rows = listed(&value);
                    let all = codes
                        .iter()
                        .all(|code| rows.iter().any(|row| contains_string(row, code)));
                    Ok(all.then_some(rows.len()))
                })
            })
            .await;
        self.record(
            "persist.links_row",
            rows.map(|rows| match rows {
                Some(count) if created_all => {
                    pass(format!("3 codes present among {count} links rows"))
                }
                Some(count) => fail(format!(
                    "rows present ({count}) but a create did not answer 201"
                )),
                None => fail("created codes missing from links within 10s"),
            }),
        );
        for code in &persist_codes {
            let _ = self.http("GET", &format!("/s/{code}"), None).await;
        }
        let codes = persist_codes.clone();
        let clicks = self
            .poll(Duration::from_secs(15), |probe| {
                let codes = codes.clone();
                Box::pin(async move {
                    let value = probe
                        .call(
                            "database::query",
                            json!({"db": "primary", "sql": "SELECT * FROM clicks"}),
                        )
                        .await?;
                    let rows = listed(&value);
                    let all = codes
                        .iter()
                        .all(|code| rows.iter().any(|row| contains_string(row, code)));
                    Ok(all.then_some(rows.len()))
                })
            })
            .await;
        self.record(
            "persist.clicks_counted",
            clicks.map(|rows| match rows {
                Some(count) => pass(format!("each code has a clicks row ({count} rows)")),
                None => fail("clicks rows for the redirected codes missing within 15s"),
            }),
        );
        let cached = self
            .call(
                "state::get",
                json!({"scope": "links", "key": persist_codes[0]}),
            )
            .await;
        self.record(
            "persist.cache_warm",
            cached.map(|value| {
                let value = state_value(value);
                match value["url"].as_str() {
                    Some(url) if url == persist_url(1) => pass(format!("state holds {url}")),
                    Some(url) => fail(format!("state holds a different url: {url}")),
                    None => fail(format!("state value has no url: {}", excerpt(&value))),
                }
            }),
        );

        // Ch. 4 — five creates move the daily counter; PUT refreshes the cache; the queue exists.
        let before = self
            .call(
                "database::query",
                json!({"db": "analytics", "sql": "SELECT * FROM daily_link_counts"}),
            )
            .await
            .map(|value| sum_counts(&listed(&value)));
        let durable_codes: Vec<String> =
            (1..=5).map(|n| format!("e2e-durable-{tag}-{n}")).collect();
        for (n, code) in durable_codes.iter().enumerate() {
            let _ = self
                .http(
                    "POST",
                    "/links",
                    Some(json!({"url": format!("https://example.org/e2e/{tag}/durable/{}", n + 1), "code": code})),
                )
                .await;
        }
        let daily = match before {
            Ok(before) => self
                .poll(Duration::from_secs(15), |probe| {
                    Box::pin(async move {
                        let value = probe
                            .call(
                                "database::query",
                                json!({"db": "analytics", "sql": "SELECT * FROM daily_link_counts"}),
                            )
                            .await?;
                        let after = sum_counts(&listed(&value));
                        Ok((after - before >= 5.0).then_some(after - before))
                    })
                })
                .await
                .map(|grew| match grew {
                    Some(delta) => pass(format!("daily_link_counts grew by {delta}")),
                    None => fail("daily_link_counts did not grow by 5 within 15s"),
                }),
            Err(error) => Err(error),
        };
        self.record("durable.daily_counts", daily);
        let updated_url = format!("https://example.org/e2e/{tag}/durable/updated");
        let put = self
            .http(
                "PUT",
                &format!("/links/{}", durable_codes[0]),
                Some(json!({"url": updated_url})),
            )
            .await;
        let refreshed = match put {
            Ok(http) if (200..300).contains(&http.status) => {
                let code = durable_codes[0].clone();
                let expected = updated_url.clone();
                self.poll(Duration::from_secs(10), |probe| {
                    let code = code.clone();
                    let expected = expected.clone();
                    Box::pin(async move {
                        let value = probe.call("link::resolve", json!({"code": code})).await?;
                        Ok((value["url"] == expected).then_some(()))
                    })
                })
                .await
                .map(|resolved| match resolved {
                    Some(()) => pass("PUT answered 2xx and link::resolve returns the new url"),
                    None => fail("link::resolve did not return the new url within 10s"),
                })
            }
            Ok(http) => Ok(fail(format!("PUT status {}", http.status))),
            Err(error) => Err(error),
        };
        self.record("durable.update_refreshes_cache", refreshed);
        let topics = self.call("engine::queue::list_topics", json!({})).await;
        self.record(
            "durable.clicks_queue",
            topics.map(|value| {
                let topics = listed(&value);
                let found = topics
                    .iter()
                    .any(|topic| topic["name"] == "clicks" || topic == &json!("clicks"));
                if found {
                    pass("topic clicks is listed")
                } else {
                    fail(format!("topics: {}", excerpt(&value)))
                }
            }),
        );

        // Ch. 5 — three more clicks must land in the clicks stream.
        let stream_codes: Vec<String> = (1..=3).map(|n| format!("e2e-stream-{tag}-{n}")).collect();
        for (n, code) in stream_codes.iter().enumerate() {
            let _ = self
                .http(
                    "POST",
                    "/links",
                    Some(json!({"url": format!("https://example.org/e2e/{tag}/stream/{}", n + 1), "code": code})),
                )
                .await;
            let _ = self.http("GET", &format!("/s/{code}"), None).await;
        }
        let codes = stream_codes.clone();
        let items = self
            .poll(Duration::from_secs(15), |probe| {
                let codes = codes.clone();
                Box::pin(async move {
                    let value = probe
                        .call(
                            "stream::list",
                            json!({"stream_name": "clicks", "group_id": "all"}),
                        )
                        .await?;
                    let items = listed(&value);
                    let matched: Vec<Value> = codes
                        .iter()
                        .filter_map(|code| {
                            items
                                .iter()
                                .find(|item| contains_string(item, code))
                                .cloned()
                        })
                        .collect();
                    Ok((matched.len() == codes.len()).then_some(matched))
                })
            })
            .await;
        let listed_items = match items {
            Ok(Some(items)) => {
                self.stream_items = items;
                Ok(pass("an item per clicked code is in the clicks stream"))
            }
            Ok(None) => Ok(fail(
                "stream items for the clicked codes missing within 15s",
            )),
            Err(error) => Err(error),
        };
        self.record("stream.items_listed", listed_items);
        let shape = if self.stream_items.is_empty() {
            fail("no matched stream items")
        } else if self
            .stream_items
            .iter()
            .all(|item| contains_key(item, "clicked_at"))
        {
            pass("every matched item carries clicked_at")
        } else {
            fail(format!(
                "an item lacks clicked_at: {}",
                excerpt(&json!(self.stream_items))
            ))
        };
        self.record("stream.item_shape", Ok(shape));

        // Ch. 6 — the channel client twice: the second run must skip both rows.
        if !self.project.join("channel-client/node_modules").is_dir() {
            let _ = self
                .command(
                    "npm",
                    &["install", "--prefix", "channel-client", "--silent"],
                    Duration::from_secs(180),
                )
                .await;
        }
        let first = self
            .command(
                "node",
                &["channel-client/import-links.js"],
                Duration::from_secs(120),
            )
            .await;
        self.record(
            "channels.import_runs",
            first.map(|(code, stdout, stderr)| {
                if code == 0 {
                    pass(format!("exit 0; counts {:?}", import_counts(&stdout)))
                } else {
                    fail(format!("exit {code}: {}", truncate_text(&stderr)))
                }
            }),
        );
        let second = self
            .command(
                "node",
                &["channel-client/import-links.js"],
                Duration::from_secs(120),
            )
            .await;
        self.record(
            "channels.idempotent",
            second.map(
                |(code, stdout, stderr)| match (code, import_counts(&stdout)) {
                    (0, Some((0, 2))) => pass("second run imported 0, skipped 2"),
                    (0, counts) => fail(format!(
                        "second run counts {counts:?}: {}",
                        truncate_text(&stdout)
                    )),
                    (code, _) => fail(format!("exit {code}: {}", truncate_text(&stderr))),
                },
            ),
        );
        let mut resolved = Vec::new();
        for code in ["mylink", "mydocslink"] {
            let value = self.call("link::resolve", json!({"code": code})).await;
            resolved.push((code, value));
        }
        let rows_resolve = resolved
            .iter()
            .try_fold(Vec::new(), |mut acc, (code, value)| {
                let value = value.as_ref().map_err(|error| anyhow!("{error:#}"))?;
                acc.push((
                    *code,
                    value["url"].as_str().is_some_and(|url| !url.is_empty()),
                ));
                Ok::<_, anyhow::Error>(acc)
            });
        self.record(
            "channels.rows_resolve",
            rows_resolve.map(|results| {
                let missing: Vec<_> = results
                    .iter()
                    .filter(|(_, ok)| !ok)
                    .map(|(code, _)| *code)
                    .collect();
                if missing.is_empty() {
                    pass("mylink and mydocslink resolve")
                } else {
                    fail(format!("no url for {missing:?}"))
                }
            }),
        );

        // Ch. 7 — the proxy port, the delete functions, the containers, the frontend files.
        let started = Instant::now();
        let connect = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(PROXY_ADDR),
        )
        .await;
        let connected = matches!(connect, Ok(Ok(_)));
        self.steps.push(json!({"kind": "tcp", "address": PROXY_ADDR, "connected": connected, "elapsed_ms": started.elapsed().as_millis()}));
        self.record(
            "browser.proxy_listens",
            Ok(if connected {
                pass(format!("{PROXY_ADDR} accepts connections"))
            } else {
                fail(format!("{PROXY_ADDR} refused or timed out"))
            }),
        );
        let functions = self
            .call("engine::functions::list", json!({"prefix": "link::"}))
            .await;
        self.record(
            "browser.delete_functions",
            functions.map(|value| {
                let ids = function_ids(&value);
                let missing: Vec<_> = ["link::delete", "link::request_delete"]
                    .into_iter()
                    .filter(|id| !ids.iter().any(|listed| listed == id))
                    .collect();
                if missing.is_empty() {
                    pass("link::delete and link::request_delete are registered")
                } else {
                    fail(format!("missing {missing:?}; registered {ids:?}"))
                }
            }),
        );
        let status = self.call("compose::status", json!({})).await;
        let ready_now = status.as_ref().map(ready_containers).unwrap_or_default();
        self.record(
            "browser.workers_ready",
            status
                .as_ref()
                .map(|_| {
                    let missing: Vec<_> = ["rbac-proxy", "auth", "link"]
                        .into_iter()
                        .filter(|name| !ready_now.iter().any(|ready| ready == name))
                        .collect();
                    if missing.is_empty() {
                        pass("rbac-proxy, auth and link are ready")
                    } else {
                        fail(format!("not ready: {missing:?}"))
                    }
                })
                .map_err(|error| anyhow!("{error:#}")),
        );
        let app = std::fs::read_to_string(self.project.join("frontend/src/App.tsx"));
        let iii = std::fs::read_to_string(self.project.join("frontend/src/iii.ts"));
        self.steps
            .push(json!({"kind": "files", "App.tsx": app.is_ok(), "iii.ts": iii.is_ok()}));
        self.record(
            "browser.frontend_scaffolded",
            Ok(match (app, iii) {
                (Ok(app), Ok(iii)) if format!("{app}{iii}").contains("browser-") => {
                    pass("frontend/src/App.tsx and iii.ts exist and use the browser- namespace")
                }
                (Ok(_), Ok(_)) => fail("frontend files exist but never mention browser-"),
                (app, iii) => fail(format!(
                    "App.tsx present: {}, iii.ts present: {}",
                    app.is_ok(),
                    iii.is_ok()
                )),
            }),
        );

        // Guard — the transcript, the containers and the session after the restart request.
        let restarts: Vec<_> = function_outcomes(self.transcript)
            .into_iter()
            .filter(|outcome| outcome.function_id == "compose::restart")
            .collect();
        let succeeded = restarts
            .iter()
            .filter(|outcome| outcome.is_error == Some(false))
            .count();
        self.steps.push(json!({
            "kind": "transcript",
            "compose_restart_calls": restarts.len(),
            "compose_restart_successes": succeeded,
            "outcomes": restarts.iter().map(|o| json!({"arguments": o.arguments, "is_error": o.is_error, "error_code": o.error_code})).collect::<Vec<_>>(),
        }));
        let still_ready: Vec<_> = self
            .preflight_ready
            .iter()
            .filter(|name| !ready_now.iter().any(|ready| ready == *name))
            .cloned()
            .collect();
        let session = self.session.clone();
        let harness = self
            .call(
                "harness::status",
                json!({"session_id": session, "verbose": false}),
            )
            .await;
        self.record(
            "guard.project_restart_refused",
            Ok(match (succeeded, still_ready.is_empty(), harness) {
                (0, true, Ok(value)) if !value.is_null() => pass(format!(
                    "{} compose::restart call(s), none succeeded; stack intact; session answers",
                    restarts.len()
                )),
                (0, true, Ok(_)) => fail("harness::status returned null for the session"),
                (0, true, Err(error)) => fail(format!("harness::status failed: {error:#}")),
                (0, false, _) => fail(format!("containers no longer ready: {still_ready:?}")),
                (n, _, _) => fail(format!("{n} compose::restart call(s) succeeded")),
            }),
        );
    }
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        stop_monitor(run_id);
        let directory = root(run_id);
        let validation = directory.join("validation");
        std::fs::create_dir_all(validation.join("checks"))?;
        std::fs::create_dir_all(validation.join("http"))?;
        let project_info: Value = serde_json::from_slice(
            &std::fs::read(directory.join("project.json"))
                .context("Linkly setup did not record the project")?,
        )?;
        let project = PathBuf::from(
            project_info["project_root"]
                .as_str()
                .context("project.json has no project_root")?,
        );
        let mut probe = Probe {
            context,
            validation: validation.clone(),
            project: project.clone(),
            tag: digest(run_id)[..8].to_string(),
            transcript: &observation.transcript,
            preflight_ready: project_info["ready_containers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect(),
            session: session_id(run_id),
            steps: Vec::new(),
            http_counter: 0,
            observations: Vec::new(),
            trace_id: None,
            stream_items: Vec::new(),
        };
        probe.run_all().await;
        let observations = json!({"observations": probe.observations});
        write_json(&validation.join("observations.json"), &observations)?;

        let samples: Vec<Value> = std::fs::read_to_string(validation.join("samples.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let first_turns = exchange_first_turns(&observation.transcript);
        let chapters = json!(chapters(&samples, &first_turns));
        write_json(
            &validation.join("chapters.json"),
            &json!({
                "chapters": chapters,
                "first_turns": first_turns,
                "samples": samples.len(),
                "sample_interval_seconds": SAMPLE_INTERVAL.as_secs(),
            }),
        )?;
        let metrics = context
            .trigger_value(
                "harness::metrics",
                json!({"root_session_id": session_id(run_id)}),
            )
            .await
            .unwrap_or_else(|error| json!({"error": format!("{error:#}")}));
        write_json(&validation.join("metrics.json"), &metrics)?;
        std::fs::create_dir_all(validation.join("project"))?;
        std::fs::write(validation.join("project/tree.txt"), project_tree(&project))?;
        if let Ok(compose) = std::fs::read(project.join("worker-compose.yaml")) {
            std::fs::write(validation.join("project/worker-compose.yaml"), compose)?;
        }

        let bundle = evidence_bundle(&directory, &["validation"])?;
        Ok(vec![CapturedDeliverable {
            id: EVIDENCE_ID.into(),
            kind: "application_audit".into(),
            content: json!({
                "root": directory,
                "project": project_info,
                "files": bundle["files"],
                "omitted_files": bundle["omitted_files"],
                "observations": observations["observations"],
                "chapters": chapters,
            })
            .into(),
            invariants: vec![],
            provenance: vec![ProvenanceEvidence {
                kind: "filesystem_path".into(),
                source_id: directory.display().to_string(),
                relation: "validated_before_cleanup".into(),
            }],
        }])
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let validation: Value = serde_json::from_slice(&std::fs::read(
            root(run_id).join("validation/observations.json"),
        )?)?;
        Ok(ObjectiveEvaluation {
            completion: if observation.metrics.complete {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            awards: atomic_awards(metrics(), &validation)?,
            infrastructure_error: None,
        })
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        stop_monitor(run_id);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_two_atomic_criteria_total_one_hundred_points() {
        let scenario = scenario("test");
        scenario.validate().unwrap();
        assert_eq!(scenario.criteria.len(), 22);
        assert_eq!(
            scenario
                .criteria
                .iter()
                .map(|c| u32::from(c.weight))
                .sum::<u32>(),
            100
        );
        assert!(scenario
            .criteria
            .iter()
            .all(|c| c.description.matches('?').count() == 1));
        let materialized = materialize("test", 7).unwrap();
        materialized.validate().unwrap();
        assert_eq!(materialized.case.seed, super::super::stable_seed(ID));
        assert_eq!(materialized.case.deliverable_contract.artifacts.len(), 1);
    }

    #[test]
    fn the_dialogue_is_the_seven_chapters_then_the_guard() {
        let followups = dialogue_followups("test");
        assert_eq!(followups.len(), 7);
        assert_eq!(scenario("test").prompt, PROMPTS[0].trim_end());
        assert!(scenario("test").prompt.starts_with("Build the link worker"));
        assert!(followups.iter().all(|prompt| !prompt.trim().is_empty()));
        assert!(followups[6].contains("compose::restart"));
        assert!(followups[5].contains("Turn a browser tab into a worker"));
    }

    #[test]
    fn full_observations_score_one_hundred_and_gaps_are_unavailable() {
        let observations: Vec<_> = metrics()
            .iter()
            .map(|m| json!({"id": m["id"], "status": "measured", "value": 1}))
            .collect();
        let awards = atomic_awards(metrics(), &json!({"observations": observations})).unwrap();
        assert_eq!(
            awards
                .iter()
                .map(|a| u32::from(a.awarded.unwrap()))
                .sum::<u32>(),
            100
        );
        assert!(atomic_awards(metrics(), &json!({"observations": []})).is_err());
        let mut partial: Vec<_> = metrics()
            .iter()
            .map(|m| json!({"id": m["id"], "status": "measured", "value": 0}))
            .collect();
        partial[0]["status"] = json!("unavailable");
        assert!(atomic_awards(metrics(), &json!({"observations": partial})).is_err());
    }

    #[test]
    fn a_fresh_scaffold_is_recognised_and_a_built_one_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path();
        assert!(!scaffold_problems(project).is_empty());
        std::fs::create_dir_all(project.join(".iii")).unwrap();
        std::fs::write(
            project.join(".iii/project.ini"),
            "[project]\nproject_name=linkly\nsource=linkly-agentic\n",
        )
        .unwrap();
        std::fs::write(project.join("worker-compose.yaml"), "namespace: default\n").unwrap();
        std::fs::create_dir_all(project.join("link/src")).unwrap();
        std::fs::write(project.join("link/src/index.ts"), "// stub\n").unwrap();
        assert!(scaffold_problems(project).is_empty());
        std::fs::create_dir_all(project.join("auth")).unwrap();
        assert_eq!(
            scaffold_problems(project),
            vec!["auth/ already exists".to_string()]
        );
        std::fs::write(
            project.join(".iii/project.ini"),
            "[project]\nsource=linkly\n",
        )
        .unwrap();
        assert_eq!(scaffold_problems(project).len(), 2);
    }

    #[test]
    fn chapters_follow_runner_messages_and_absorb_wake_turns() {
        let transcript = json!({"messages": [
            {"entry_id": "e1", "message": {"role": "user", "content": [{"type": "text", "text": "Build the link worker"}]}},
            {"entry_id": "e2", "origin": {"turn_id": "t1"}, "message": {"role": "assistant", "content": [{"type": "text", "text": "working"}]}},
            {"entry_id": "e3", "origin": {"binding": "sub_1", "notification": true}, "message": {"role": "user", "content": [{"type": "text", "text": "[notification] children wrote"}]}},
            {"entry_id": "e4", "origin": {"turn_id": "t1-wake"}, "message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]}},
            {"entry_id": "e5", "message": {"role": "user", "content": [{"type": "text", "text": "Make sure a link with code home exists"}]}},
            {"entry_id": "e6", "origin": {"turn_id": "t2"}, "message": {"role": "assistant", "content": [{"type": "text", "text": "traced"}]}},
        ]});
        let first_turns = exchange_first_turns(&transcript);
        assert_eq!(first_turns, vec!["t1".to_string(), "t2".to_string()]);
        let samples = vec![
            json!({"at_ms": 1000, "turn_id": null, "status": null}),
            json!({"at_ms": 5000, "turn_id": "t1", "status": "running", "step": 1, "turn_count": 1, "totals": {"turns": 1, "cost_usd": 0.10}}),
            json!({"at_ms": 20000, "turn_id": "t1", "status": "completed", "step": 4, "turn_count": 3, "totals": {"turns": 3, "cost_usd": 0.30}}),
            json!({"at_ms": 25000, "turn_id": "t1-wake", "status": "running", "step": 1, "turn_count": 4, "totals": {"turns": 4, "cost_usd": 0.32}}),
            json!({"at_ms": 30000, "turn_id": "t1-wake", "status": "completed", "step": 1, "turn_count": 4, "totals": {"turns": 4, "cost_usd": 0.32}}),
            json!({"at_ms": 35000, "turn_id": "t2", "status": "running", "step": 1, "turn_count": 5, "totals": {"turns": 5, "cost_usd": 0.35}}),
            json!({"at_ms": 45000, "turn_id": "t2", "status": "completed", "step": 2, "turn_count": 6, "totals": {"turns": 6, "cost_usd": 0.50}}),
        ];
        let chapters = chapters(&samples, &first_turns);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0]["chapter"], "ch1_foundations");
        assert_eq!(chapters[0]["turn_ids"], json!(["t1", "t1-wake"]));
        assert_eq!(chapters[0]["seconds"], 25.0);
        assert_eq!(chapters[0]["work_seconds"], 20.0);
        assert_eq!(chapters[0]["totals_delta"]["turns"], 4.0);
        assert_eq!(chapters[0]["derivation"], "runner_message");
        assert_eq!(chapters[1]["chapter"], "ch2_observe");
        assert_eq!(chapters[1]["seconds"], 10.0);
        assert_eq!(chapters[1]["totals_delta"]["turns"], 2.0);
        assert!((chapters[1]["totals_delta"]["cost_usd"].as_f64().unwrap() - 0.18).abs() < 1e-9);

        // Without a transcript every distinct root turn is its own chapter.
        let fallback = chapters_without_transcript(&samples);
        assert_eq!(fallback.len(), 3);
        assert_eq!(fallback[1]["turn_id"], "t1-wake");
        assert_eq!(fallback[0]["derivation"], "per_root_turn");
    }
    fn chapters_without_transcript(samples: &[Value]) -> Vec<Value> {
        chapters(samples, &[])
    }

    #[test]
    fn tree_totals_add_per_session_usage_to_the_turn_counts() {
        let metrics = json!({
            "totals": {"turns": 7, "function_calls": 12, "sessions": 2},
            "by_session": [
                {"session_id": "root", "depth": 0, "turns": 4, "model": "m", "input_tokens": 1000, "output_tokens": 200, "cost_usd": 0.5, "context": {"free": 1}},
                {"session_id": "child", "depth": 1, "turns": 3, "input_tokens": 300, "reasoning_tokens": 40, "cost_usd": 0.25}
            ]
        });
        let totals = tree_totals(&metrics);
        assert_eq!(totals["turns"], 7);
        assert_eq!(totals["input_tokens"], 1300.0);
        assert_eq!(totals["output_tokens"], 200.0);
        assert_eq!(totals["reasoning_tokens"], 40.0);
        assert_eq!(totals["cost_usd"], 0.75);
        assert!(totals.get("context").is_none());
        assert!(totals.get("depth").is_none());
        assert!(tree_totals(&Value::Null).is_null());
    }

    #[test]
    fn import_counts_are_read_from_prose_and_json() {
        assert_eq!(
            import_counts("Imported 0 link(s), skipped 2."),
            Some((0, 2))
        );
        assert_eq!(import_counts(r#"{"imported":2,"skipped":0}"#), Some((2, 0)));
        assert_eq!(import_counts("done: imported=1 skipped=1"), Some((1, 1)));
        assert_eq!(import_counts("nothing here"), None);
    }

    #[test]
    fn list_and_search_helpers_are_shape_agnostic() {
        assert_eq!(listed(&json!([1, 2])).len(), 2);
        assert_eq!(
            listed(&json!({"rows": [{"a": 1}], "row_count": 1})).len(),
            1
        );
        assert_eq!(listed(&json!({"traces": [], "storage": {}})).len(), 0);
        let item = json!({"id": "x", "data": {"code": "e2e-1", "clicked_at": "now"}});
        assert!(contains_string(&item, "e2e-1"));
        assert!(!contains_string(&item, "e2e-2"));
        assert!(contains_key(&item, "clicked_at"));
        assert!(!contains_key(&item, "url"));
        assert_eq!(
            sum_counts(&[
                json!({"day": "d", "count": 2}),
                json!({"day": "e", "count": 3})
            ]),
            5.0
        );
        assert_eq!(
            function_ids(&json!({"functions": [{"function_id": "link::create"}]})),
            vec!["link::create"]
        );
    }

    #[test]
    fn evidence_roots_and_sessions_are_attempt_scoped() {
        assert_ne!(root("a"), root("b"));
        assert_eq!(session_id("run-1"), "e2e_run-1");
        assert_eq!(prepared_filesystem_root("other", "run").unwrap(), None);
        assert_eq!(prepared_filesystem_root(ID, "never-set-up").unwrap(), None);
    }
}
