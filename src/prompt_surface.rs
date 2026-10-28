//! The prompt surface a run starts from and the Harness configuration it runs
//! under, audited without a model call. [`preflight`] reads the stack before
//! the subject's first send and refuses a run whose Harness is not ready or is
//! not configured the way the regression measures it. [`collect`] reads each
//! session's frozen prompt, turn record and stored prompt body once the turn
//! finished, and [`violations`] judges them as Harness invariants.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::{function_ids, E2eContext};
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::E2eRunReport;
use crate::suite::SubjectConfig;

/// A Harness function or trigger type the stack does not serve (yet).
pub const HARNESS_NOT_READY: &str = "harness_not_ready";
/// A Harness configured or registered unlike the one the regression measures.
pub const HARNESS_MISCONFIGURED: &str = "harness_misconfigured";

const DEFAULT_IDENTITY_PREFIX: &str = "You are an iii agent worker.";
/// Below this many words a prompt may leave the language unpinned: the
/// Harness pins it only at 0.8 detector confidence, which short text misses
/// (the onboarding tour opens with "hey"). Every stored English prompt this
/// long pinned English.
const LANGUAGE_MIN_WORDS: usize = 20;

/// `register` in the Harness's `functions/mod.rs`: on the agent catalog.
const PUBLIC_FUNCTIONS: [&str; 7] = [
    "harness::spawn",
    "harness::ask",
    "harness::status",
    "harness::session-tree",
    "harness::metrics",
    "harness::triggers::list",
    "harness::triggers::unregister",
];
/// `register_internal` (and trace-hidden) there: registered, off the catalog.
const INTERNAL_FUNCTIONS: [&str; 24] = [
    "harness::delete-session-tree",
    "harness::delete-session-tree-status",
    "harness::delete-session-tree-run",
    "harness::send",
    "harness::compact",
    "harness::trigger::deliver",
    "harness::turn",
    "harness::function::trigger",
    "harness::function::resolve",
    "harness::stop",
    "harness::system-prompt::get",
    "harness::context-policy",
    "harness::teardown",
    "harness::unqueue",
    "harness::edit_queued",
    "harness::filesystem::grant",
    "harness::filesystem::grants",
    "harness::filesystem::revoke",
    "harness::filesystem::info",
    "harness::projects::list",
    "harness::projects::upsert",
    "harness::projects::delete",
    "harness::sweep-pending",
    "harness::on-session-deleted",
];
const TRIGGER_TYPES: [&str; 10] = [
    "harness::ready",
    "harness::turn-started",
    "harness::compaction",
    "timer",
    "harness::hook::pre-turn",
    "harness::hook::pre-generate",
    "harness::hook::post-generate",
    "harness::hook::pre-trigger",
    "harness::hook::post-trigger",
    "harness::hook::post-turn",
];
/// Keys the Harness migrates out of its entry at boot.
const RETIRED_KEYS: [&str; 3] = ["provider_identity_prompt", "permissions", "providers"];

/// What the preflight read of the stack the run measures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessPreflight {
    /// The Harness's configuration entry and the SHA-256 of its value.
    pub configuration_id: String,
    pub configuration_sha256: String,
    /// `router::route` for the subject model with no provider pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_provider: Option<String>,
    #[serde(default)]
    pub route_candidates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreflightFailure {
    /// [`HARNESS_NOT_READY`] or [`HARNESS_MISCONFIGURED`].
    pub code: &'static str,
    pub message: String,
}

impl PreflightFailure {
    fn not_ready(error: anyhow::Error) -> Self {
        Self {
            code: HARNESS_NOT_READY,
            message: format!("{error:#}"),
        }
    }
}

pub async fn preflight(
    context: &E2eContext,
    subject: &SubjectConfig,
) -> Result<HarnessPreflight, PreflightFailure> {
    let catalog = context
        .trigger_value("engine::functions::list", json!({}))
        .await
        .context("list the function catalog")
        .map_err(PreflightFailure::not_ready)?;
    let registered = context
        .trigger_value(
            "engine::functions::list",
            json!({ "include_internal": true }),
        )
        .await
        .context("list every registered function")
        .map_err(PreflightFailure::not_ready)?;
    let triggers = context
        .trigger_value("engine::triggers::list", json!({}))
        .await
        .context("list trigger types")
        .map_err(PreflightFailure::not_ready)?;
    let (missing, mut misconfigured) = surface_problems(&catalog, &registered, &triggers);
    if !missing.is_empty() {
        return Err(PreflightFailure {
            code: HARNESS_NOT_READY,
            message: format!("the Harness does not serve {}", missing.join(", ")),
        });
    }
    let (configuration_id, configuration) = harness_configuration(context)
        .await
        .map_err(PreflightFailure::not_ready)?;
    misconfigured.extend(configuration_problems(&configuration));
    let policy = context
        .trigger_value("harness::context-policy", json!({ "model": subject.model }))
        .await
        .context("read the context policy")
        .map_err(PreflightFailure::not_ready)?;
    // The regression's pruning measurements assume a model whose aged results
    // may be pruned; a thinking-binding model would never prune.
    if policy.get("allow_prune") != Some(&Value::Bool(true)) {
        misconfigured.push(format!(
            "harness::context-policy does not allow pruning for {}",
            subject.model
        ));
    }
    if !misconfigured.is_empty() {
        return Err(PreflightFailure {
            code: HARNESS_MISCONFIGURED,
            message: misconfigured.join("; "),
        });
    }
    let route = context
        .trigger_value("router::route", json!({ "model": subject.model }))
        .await
        .context("route the subject model")
        .map_err(PreflightFailure::not_ready)?;
    Ok(HarnessPreflight {
        configuration_id,
        configuration_sha256: crate::artifact::sha256_value(&configuration)
            .map_err(PreflightFailure::not_ready)?,
        route_provider: route
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        route_candidates: route
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    })
}

/// The entry the Harness registered for itself (`metadata.ui_form`
/// "harness"): its id carries the stack's namespace prefix.
async fn harness_configuration(context: &E2eContext) -> Result<(String, Value)> {
    let listed = context
        .trigger_value("configuration::list", json!({}))
        .await
        .context("list configuration entries")?;
    let id = listed
        .get("configurations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.pointer("/metadata/ui_form").and_then(Value::as_str) == Some("harness"))
        .and_then(|entry| entry.get("id").and_then(Value::as_str))
        .context("no Harness configuration entry is registered")?
        .to_string();
    let value = context
        .trigger_value("configuration::get", json!({ "id": id }))
        .await
        .with_context(|| format!("read configuration {id}"))?
        .get("value")
        .cloned()
        .with_context(|| format!("configuration {id} has no value"))?;
    Ok((id, value))
}

/// Functions and trigger types that are missing (not ready), and internal
/// functions the default catalog lists (misconfigured).
fn surface_problems(
    catalog: &Value,
    registered: &Value,
    triggers: &Value,
) -> (Vec<String>, Vec<String>) {
    let catalog = function_ids(catalog).collect::<BTreeSet<_>>();
    let registered = function_ids(registered).collect::<BTreeSet<_>>();
    let triggers = function_ids(triggers).collect::<BTreeSet<_>>();
    let missing = PUBLIC_FUNCTIONS
        .iter()
        .filter(|id| !catalog.contains(**id))
        .chain(
            INTERNAL_FUNCTIONS
                .iter()
                .filter(|id| !registered.contains(**id)),
        )
        .map(|id| format!("function {id}"))
        .chain(
            TRIGGER_TYPES
                .iter()
                .filter(|id| !triggers.contains(**id))
                .map(|id| format!("trigger type {id}")),
        )
        .collect();
    let leaked = INTERNAL_FUNCTIONS
        .iter()
        .filter(|id| catalog.contains(**id))
        .map(|id| format!("internal function {id} is on the default catalog"))
        .collect();
    (missing, leaked)
}

/// Retired keys still set, and knobs the regression depends on off the values
/// it accepts. A knob the entry leaves out takes the Harness default.
fn configuration_problems(value: &Value) -> Vec<String> {
    // The default, plus the stamp scripts/exact_stack_campaign.py puts on a
    // stack that runs fanout_ladder (max_children 16) or depth_ladder (max_depth 6).
    let knobs = [
        ("max_validation_retries", vec![json!(2)]),
        ("max_children", vec![json!(8), json!(16)]),
        ("max_depth", vec![json!(3), json!(6)]),
        ("default_max_turns", vec![json!(500)]),
        ("prompt_cache_sections", vec![json!(true)]),
        ("co_author", vec![json!(true)]),
    ];
    let retired = RETIRED_KEYS
        .iter()
        .filter(|key| value.get(**key).is_some())
        .map(|key| format!("retired key {key} is still configured"));
    let drifted = knobs.into_iter().filter_map(|(key, accepted)| {
        let actual = value.get(key)?;
        (!accepted.contains(actual)).then(|| format!("{key} is {actual}"))
    });
    retired.chain(drifted).collect()
}

/// The evidence [`violations`] judges, read before teardown; never persisted.
#[derive(Debug, Clone, Default)]
pub struct PromptSurface {
    /// `harness::system-prompt::get` `default_only`: the effective identity.
    pub default_prompt: Option<String>,
    /// The subject's agent profile prompt, when it runs as one.
    pub profile_prompt: Option<String>,
    /// `harness::filesystem::info`.
    pub default_root: Option<String>,
    pub configured_roots: bool,
    /// The `fs_scope.root` the suite sent with the root session.
    pub sent_root: Option<String>,
    /// The root session's `metadata.response_language.code`.
    pub response_language: Option<String>,
    /// The root first, then each session `metrics.by_session` names.
    pub sessions: Vec<SessionSurface>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionSurface {
    pub session_id: String,
    pub transcript: Value,
    /// The stored `harness_turn` record, prompt refs in place.
    pub record: Value,
    /// The `harness_prompt` body the record's prompt ref names.
    pub prompt_body: Option<String>,
    /// The built-in layer `harness::system-prompt::get` reports as frozen.
    pub frozen_prompt: Option<String>,
}

pub async fn collect(
    context: &E2eContext,
    subject: &SubjectConfig,
    report: &E2eRunReport,
    sent_root: Option<&str>,
) -> Result<PromptSurface> {
    let root = report.session_id.as_str();
    let built_in = |preview: Value| {
        preview
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|part| part.get("kind").and_then(Value::as_str) == Some("built_in"))
            .and_then(|part| part.get("body").and_then(Value::as_str))
            .map(str::to_string)
    };
    let default_prompt = built_in(
        context
            .trigger_value(
                "harness::system-prompt::get",
                json!({ "session_id": root, "default_only": true }),
            )
            .await
            .context("read the default system prompt")?,
    );
    let profile_prompt = match subject.agent.as_deref() {
        Some(id) => context
            .trigger_value("directory::agents::get", json!({ "id": id }))
            .await
            .with_context(|| format!("read agent profile {id}"))?
            .get("system_prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
        None => None,
    };
    let filesystem = context
        .trigger_value("harness::filesystem::info", json!({}))
        .await
        .context("read the Harness filesystem defaults")?;
    let session = context
        .trigger_value("session::get", json!({ "session_id": root }))
        .await
        .context("read the root session")?;
    let mut ids = vec![root.to_string()];
    for usage in report
        .metrics
        .iter()
        .flat_map(|metrics| &metrics.by_session)
    {
        if !ids.contains(&usage.session_id) {
            ids.push(usage.session_id.clone());
        }
    }
    let mut sessions = Vec::with_capacity(ids.len());
    for session_id in ids {
        let transcript = match (&report.transcript, session_id == root) {
            (Some(transcript), true) => transcript.clone(),
            _ => context.transcript(&session_id).await?,
        };
        let record = context
            .trigger_value(
                "state::get",
                json!({ "scope": "harness_turn", "key": session_id }),
            )
            .await
            .with_context(|| format!("read the turn record of {session_id}"))?;
        let prompt_body = match prompt_ref(&record) {
            Some(digest) => context
                .trigger_value(
                    "state::get",
                    json!({ "scope": "harness_prompt", "key": digest }),
                )
                .await
                .with_context(|| format!("read prompt {digest}"))?
                .get("body")
                .and_then(Value::as_str)
                .map(str::to_string),
            None => None,
        };
        let frozen_prompt = built_in(
            context
                .trigger_value(
                    "harness::system-prompt::get",
                    json!({ "session_id": session_id }),
                )
                .await
                .with_context(|| format!("read the frozen prompt of {session_id}"))?,
        );
        sessions.push(SessionSurface {
            session_id,
            transcript,
            record,
            prompt_body,
            frozen_prompt,
        });
    }
    Ok(PromptSurface {
        default_prompt,
        profile_prompt,
        default_root: text(&filesystem, "/default_root").map(str::to_string),
        configured_roots: text(&filesystem, "/boundary") == Some("configured_roots"),
        sent_root: sent_root.map(str::to_string),
        response_language: text(&session, "/meta/metadata/response_language/code")
            .map(str::to_string),
        sessions,
    })
}

pub fn violations(report: &E2eRunReport, subject: &SubjectConfig) -> Vec<HarnessViolation> {
    let Some(surface) = report.prompt_surface.as_ref() else {
        return Vec::new();
    };
    let plain_spawns = plain_spawns(&surface.sessions);
    let mut violations = Vec::new();
    for (index, session) in surface.sessions.iter().enumerate() {
        let root = index == 0;
        violations.extend(frozen_prompt(
            surface,
            session,
            root,
            subject,
            &plain_spawns,
        ));
        violations.extend(runtime_context(surface, session, root));
        violations.extend(stored_prompt(session));
    }
    violations.extend(response_language(surface, &report.prompt));
    violations
}

/// The root runs the default identity, or its agent profile's prompt; a child
/// does only when its spawn named no `agent` and no `options.system_prompt`.
fn frozen_prompt(
    surface: &PromptSurface,
    session: &SessionSurface,
    root: bool,
    subject: &SubjectConfig,
    plain_spawns: &BTreeSet<String>,
) -> Option<HarnessViolation> {
    let (expected, source) = match (root, subject.agent.as_deref()) {
        (true, Some(agent)) => (surface.profile_prompt.as_deref(), format!("agent {agent}")),
        (true, None) => (surface.default_prompt.as_deref(), "default".into()),
        (false, _) if plain_spawns.contains(&session.session_id) => {
            (surface.default_prompt.as_deref(), "default".into())
        }
        (false, _) => return None,
    };
    let frozen = session.frozen_prompt.as_deref();
    let mut problems = Vec::new();
    if frozen != expected || frozen.is_none() {
        problems.push(format!(
            "froze {}, not the {source} prompt {}",
            digest(frozen),
            digest(expected)
        ));
    }
    if source == "default" && !frozen.is_some_and(|body| body.starts_with(DEFAULT_IDENTITY_PREFIX))
    {
        problems.push(format!("does not open with `{DEFAULT_IDENTITY_PREFIX}`"));
    }
    violation(
        HarnessInvariant::FrozenPromptDrift,
        &session.session_id,
        problems,
    )
}

/// The aid the first step froze into the system prompt (`turn_loop.rs`
/// `runtime_context_aid`), line by line.
fn runtime_context(
    surface: &PromptSurface,
    session: &SessionSurface,
    root: bool,
) -> Option<HarnessViolation> {
    let id = session.session_id.as_str();
    let aid = messages(&session.transcript)
        .iter()
        .find(|entry| text(entry, "/custom/custom_type") == Some("runtime_context"))
        .and_then(|entry| text(entry, "/custom/data/aid"));
    let Some(aid) = aid else {
        return violation(
            HarnessInvariant::RuntimeContextMalformed,
            id,
            vec!["has no runtime_context entry".into()],
        );
    };
    // A seeded <preloaded_functions> block follows the lines after a blank line.
    let lines = aid
        .split("\n\n")
        .next()
        .unwrap_or_default()
        .lines()
        .collect::<Vec<_>>();
    let mut problems = Vec::new();
    let first = format!("Your session id is {id}.");
    if lines.first() != Some(&first.as_str()) {
        problems.push(format!("opens with {:?}", lines.first().unwrap_or(&"")));
    }
    let record_root = text(&session.record, "/options/metadata/fs_scope/root");
    let directory = if root {
        surface.sent_root.as_deref()
    } else {
        record_root
    }
    .or(surface.default_root.as_deref());
    let suffix = if surface.configured_roots {
        " (default directory, not an access boundary)"
    } else {
        ""
    };
    let expected =
        directory.map(|directory| format!("Your working directory is {directory}{suffix}."));
    let actual = lines
        .iter()
        .find(|line| line.starts_with("Your working directory is "));
    if actual.copied() != expected.as_deref() {
        problems.push(format!(
            "names working directory {actual:?}, not {expected:?}"
        ));
    }
    let co_authors = lines
        .iter()
        .filter(|line| line.contains("Co-authored-by"))
        .count();
    if co_authors != 1 {
        problems.push(format!("has {co_authors} Co-authored-by lines"));
    }
    // `policy_aid`: no line for a `*` allow, one for anything narrower.
    let narrowed = !session
        .record
        .pointer("/options/functions/allow")
        .and_then(Value::as_array)
        .is_some_and(|allow| allow.iter().any(|glob| glob == "*"));
    let policy_line = lines.iter().any(|line| {
        line.starts_with("Your dispatch policy allows ONLY")
            || line.starts_with("Function dispatch is entirely disabled")
    });
    if narrowed != policy_line {
        problems.push(if narrowed {
            "has no dispatch-policy line for a narrowed policy".into()
        } else {
            "has a dispatch-policy line for an unrestricted policy".into()
        });
    }
    violation(HarnessInvariant::RuntimeContextMalformed, id, problems)
}

/// The record names its prompt by reference, and the stored body hashes to it
/// (`state.rs` `prompt_digest`).
fn stored_prompt(session: &SessionSurface) -> Option<HarnessViolation> {
    let problem = match (prompt_ref(&session.record), session.prompt_body.as_deref()) {
        (None, _) => "stores options.system_prompt without a sha256 $ref".to_string(),
        (Some(digest), None) => format!("references {digest}, which harness_prompt does not hold"),
        (Some(digest), Some(body)) => {
            let actual = crate::artifact::sha256_bytes(body.as_bytes());
            if actual == digest {
                return None;
            }
            format!("references {digest}, but its body hashes to {actual}")
        }
    };
    violation(
        HarnessInvariant::PromptRefBroken,
        &session.session_id,
        vec![problem],
    )
}

/// Every scenario prompt is English: a pinned language must be English, and a
/// prompt long enough to clear the detector must have pinned one.
fn response_language(surface: &PromptSurface, prompt: &str) -> Option<HarnessViolation> {
    let root = surface.sessions.first()?;
    let words = prompt
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphabetic))
        .count();
    let problem = match surface.response_language.as_deref() {
        Some("eng") => return None,
        Some(code) => format!("pinned response language {code} on an English prompt"),
        None if words >= LANGUAGE_MIN_WORDS => {
            format!("pinned no response language on a {words}-word English prompt")
        }
        None => return None,
    };
    violation(
        HarnessInvariant::ResponseLanguageMispinned,
        &root.session_id,
        vec![problem],
    )
}

/// Children spawned with neither `agent` nor `options.system_prompt`, from the
/// `harness::spawn` calls and results in every transcript the run read.
fn plain_spawns(sessions: &[SessionSurface]) -> BTreeSet<String> {
    let messages = sessions
        .iter()
        .flat_map(|session| messages(&session.transcript))
        .collect::<Vec<_>>();
    messages
        .iter()
        .filter(|entry| text(entry, "/message/role") == Some("assistant"))
        .flat_map(|entry| {
            entry
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("function_call"))
        .filter_map(|call| {
            let arguments = call.get("arguments")?;
            // The agent_trigger meta-function wraps the target and its payload.
            let payload = match text(call, "/function_id")? {
                "harness::spawn" => arguments,
                "agent_trigger" if text(arguments, "/function") == Some("harness::spawn") => {
                    arguments.get("payload")?
                }
                _ => return None,
            };
            let plain = payload.get("agent").is_none_or(Value::is_null)
                && payload
                    .pointer("/options/system_prompt")
                    .is_none_or(Value::is_null);
            let id = text(call, "/id")?;
            plain.then_some(id)
        })
        .filter_map(|call_id| {
            messages.iter().find_map(|entry| {
                (text(entry, "/message/role") == Some("function_result")
                    && text(entry, "/message/function_call_id") == Some(call_id))
                .then(|| text(entry, "/message/details/child_session_id"))?
            })
        })
        .map(str::to_string)
        .collect()
}

fn prompt_ref(record: &Value) -> Option<&str> {
    text(record, "/options/system_prompt/$ref").filter(|digest| digest.starts_with("sha256:"))
}

fn digest(body: Option<&str>) -> String {
    body.map_or("no prompt".into(), |body| {
        crate::artifact::sha256_bytes(body.as_bytes())
    })
}

fn violation(
    invariant: HarnessInvariant,
    session_id: &str,
    problems: Vec<String>,
) -> Option<HarnessViolation> {
    (!problems.is_empty()).then(|| HarnessViolation {
        invariant,
        detail: format!("session {session_id} {}", problems.join("; ")),
    })
}

fn messages(transcript: &Value) -> &[Value] {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::report::RunStatus;

    const DEFAULT: &str = "You are an iii agent worker.\n\nBe useful.";

    fn subject(agent: Option<&str>) -> SubjectConfig {
        SubjectConfig {
            model: "deepseek-flash".into(),
            provider: "deepseek".into(),
            agent: agent.map(str::to_string),
            thinking_level: None,
            provider_options: None,
            priced: None,
            context_window: None,
        }
    }

    fn aid(session_id: &str, policy: bool) -> String {
        let mut lines = vec![
            format!("Your session id is {session_id}."),
            "Your working directory is /work (default directory, not an access boundary)."
                .to_string(),
            "Response language: English (from the user's first message).".to_string(),
        ];
        if policy {
            lines.push("Your dispatch policy allows ONLY these functions: state::get.".into());
        }
        lines.push(
            "End every git commit message you write with a blank line, then the trailer \
             `Co-authored-by: iii harness (deepseek-flash) <noreply@iii.dev>`."
                .into(),
        );
        lines.join("\n")
    }

    fn session(session_id: &str, aid: &str, allow: &str, extra: Vec<Value>) -> SessionSurface {
        let mut messages = vec![json!({ "entry_id": "e_0_runtime_context", "custom": {
            "custom_type": "runtime_context", "data": { "aid": aid },
        } })];
        messages.extend(extra);
        SessionSurface {
            session_id: session_id.into(),
            transcript: json!({ "messages": messages }),
            record: json!({ "options": {
                "system_prompt": { "$ref": crate::artifact::sha256_bytes(DEFAULT.as_bytes()) },
                "functions": { "allow": [allow] },
                "metadata": { "fs_scope": { "root": "/work" } },
            } }),
            prompt_body: Some(DEFAULT.into()),
            frozen_prompt: Some(DEFAULT.into()),
        }
    }

    fn surface(sessions: Vec<SessionSurface>) -> PromptSurface {
        PromptSurface {
            default_prompt: Some(DEFAULT.into()),
            profile_prompt: None,
            default_root: Some("/work".into()),
            configured_roots: true,
            sent_root: None,
            response_language: Some("eng".into()),
            sessions,
        }
    }

    fn report(surface: PromptSurface) -> E2eRunReport {
        let mut report = E2eRunReport::new(
            "run".into(),
            "a".into(),
            1,
            "root".into(),
            "Schedule the release review for Thursday afternoon and confirm the attendees \
             with the owner before you send the invitation to everyone on the list."
                .into(),
        );
        report.status = RunStatus::Passed;
        report.prompt_surface = Some(surface);
        report
    }

    fn invariants(violations: &[HarnessViolation]) -> Vec<HarnessInvariant> {
        violations.iter().map(|v| v.invariant).collect()
    }

    #[test]
    fn a_default_surface_breaks_no_invariant() {
        let report = report(surface(vec![session(
            "root",
            &aid("root", false),
            "*",
            vec![],
        )]));

        assert_eq!(violations(&report, &subject(None)), []);
    }

    #[test]
    fn a_root_that_froze_another_prompt_drifted() {
        let mut root = session("root", &aid("root", false), "*", vec![]);
        root.frozen_prompt = Some("You are CUSTOM.".into());

        let violations = violations(&report(surface(vec![root])), &subject(None));

        assert_eq!(
            invariants(&violations),
            [HarnessInvariant::FrozenPromptDrift]
        );
        assert!(violations[0].detail.contains("does not open with"));
    }

    #[test]
    fn an_agent_subject_freezes_its_profile_prompt() {
        let mut root = session("root", &aid("root", false), "*", vec![]);
        root.frozen_prompt = Some("Profile.".into());
        let mut matching = surface(vec![root.clone()]);
        matching.profile_prompt = Some("Profile.".into());
        let mut other = surface(vec![root]);
        other.profile_prompt = Some("Another profile.".into());

        assert_eq!(violations(&report(matching), &subject(Some("lead"))), []);
        assert_eq!(
            invariants(&violations(&report(other), &subject(Some("lead")))),
            [HarnessInvariant::FrozenPromptDrift]
        );
    }

    fn spawn(call_id: &str, child: &str, payload: Value) -> Vec<Value> {
        vec![
            json!({ "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": call_id, "function_id": "agent_trigger",
                "arguments": { "function": "harness::spawn", "payload": payload },
            }] } }),
            json!({ "message": {
                "role": "function_result", "function_call_id": call_id,
                "function_id": "harness::spawn",
                "details": { "child_session_id": child },
            } }),
        ]
    }

    #[test]
    fn only_a_plain_spawn_holds_its_child_to_the_default_prompt() {
        let mut spawns = spawn("c1", "plain", json!({ "task": "t" }));
        spawns.extend(spawn(
            "c2",
            "custom",
            json!({ "task": "t", "options": { "system_prompt": "Custom." } }),
        ));
        spawns.extend(spawn(
            "c3",
            "profiled",
            json!({ "task": "t", "agent": "lead" }),
        ));
        let root = session("root", &aid("root", false), "*", spawns);
        let children = ["plain", "custom", "profiled"].map(|id| {
            let mut child = session(id, &aid(id, false), "*", vec![]);
            child.frozen_prompt = Some("Custom.".into());
            child
        });
        let mut sessions = vec![root];
        sessions.extend(children);

        let violations = violations(&report(surface(sessions)), &subject(None));

        assert_eq!(
            invariants(&violations),
            [HarnessInvariant::FrozenPromptDrift]
        );
        assert!(violations[0].detail.starts_with("session plain "));
    }

    #[test]
    fn the_runtime_context_names_the_session_directory_co_author_and_policy() {
        let cases = [
            (aid("other", false), "*", "opens with"),
            (
                aid("root", false).replace(" (default directory, not an access boundary)", ""),
                "*",
                "names working directory",
            ),
            (
                aid("root", false).replace("Co-authored-by", "Signed-off-by"),
                "*",
                "0 Co-authored-by lines",
            ),
            (aid("root", false), "state::get", "no dispatch-policy line"),
            (
                aid("root", true),
                "*",
                "dispatch-policy line for an unrestricted",
            ),
        ];
        for (aid, allow, problem) in cases {
            let report = report(surface(vec![session("root", &aid, allow, vec![])]));

            let violations = violations(&report, &subject(None));

            assert_eq!(
                invariants(&violations),
                [HarnessInvariant::RuntimeContextMalformed],
                "{problem}"
            );
            assert!(
                violations[0].detail.contains(problem),
                "{}",
                violations[0].detail
            );
        }
    }

    #[test]
    fn the_root_directory_is_the_one_the_suite_sent() {
        let aid = aid("root", false).replace("/work", "/fixture");
        let mut sent = surface(vec![session("root", &aid, "*", vec![])]);
        sent.sent_root = Some("/fixture".into());
        let defaulted = surface(vec![session("root", &aid, "*", vec![])]);

        assert_eq!(violations(&report(sent), &subject(None)), []);
        assert_eq!(
            invariants(&violations(&report(defaulted), &subject(None))),
            [HarnessInvariant::RuntimeContextMalformed]
        );
    }

    #[test]
    fn a_prompt_is_stored_by_a_ref_its_body_hashes_to() {
        let mut inline = session("root", &aid("root", false), "*", vec![]);
        inline.record["options"]["system_prompt"] = json!(DEFAULT);
        let mut tampered = session("root", &aid("root", false), "*", vec![]);
        tampered.prompt_body = Some("You are an iii agent worker. Edited.".into());
        let mut missing = session("root", &aid("root", false), "*", vec![]);
        missing.prompt_body = None;

        for (root, problem) in [
            (inline, "without a sha256 $ref"),
            (tampered, "its body hashes to"),
            (missing, "does not hold"),
        ] {
            let violations = violations(&report(surface(vec![root])), &subject(None));

            assert_eq!(invariants(&violations), [HarnessInvariant::PromptRefBroken]);
            assert!(violations[0].detail.contains(problem));
        }
    }

    #[test]
    fn an_english_prompt_pins_english_once_it_is_long_enough() {
        let root = || session("root", &aid("root", false), "*", vec![]);
        let mut spanish = surface(vec![root()]);
        spanish.response_language = Some("spa".into());
        let mut unpinned = surface(vec![root()]);
        unpinned.response_language = None;
        let mut short = report(unpinned.clone());
        short.prompt = "hey".into();

        assert_eq!(
            invariants(&violations(&report(spanish), &subject(None))),
            [HarnessInvariant::ResponseLanguageMispinned]
        );
        assert_eq!(
            invariants(&violations(&report(unpinned), &subject(None))),
            [HarnessInvariant::ResponseLanguageMispinned]
        );
        assert_eq!(violations(&short, &subject(None)), []);
    }

    fn listed(ids: &[&str]) -> Value {
        json!({ "functions": ids.iter().map(|id| json!({ "function_id": id })).collect::<Vec<_>>() })
    }

    #[test]
    fn the_catalog_lists_public_functions_and_hides_internal_ones() {
        let all = PUBLIC_FUNCTIONS
            .iter()
            .chain(&INTERNAL_FUNCTIONS)
            .copied()
            .collect::<Vec<_>>();
        let triggers = json!({ "triggers": TRIGGER_TYPES.map(|id| json!({ "id": id })) });

        assert_eq!(
            surface_problems(&listed(&PUBLIC_FUNCTIONS), &listed(&all), &triggers),
            (vec![], vec![])
        );
        let (missing, leaked) = surface_problems(
            &listed(&["harness::spawn", "harness::send"]),
            &listed(&all[1..]),
            &json!({ "triggers": [{ "id": "harness::ready" }] }),
        );
        assert!(missing.contains(&"function harness::ask".to_string()));
        assert!(missing.contains(&"trigger type timer".to_string()));
        assert!(!missing.iter().any(|id| id.contains("harness::send")));
        assert_eq!(
            leaked,
            ["internal function harness::send is on the default catalog"]
        );
    }

    #[test]
    fn the_configuration_keeps_the_regression_defaults() {
        let live = json!({
            "max_validation_retries": 2, "max_children": 8, "max_depth": 3,
            "default_max_turns": 500, "prompt_cache_sections": true, "co_author": true,
            "router_timeout_ms": 1,
        });
        let mut stamped = live.clone();
        stamped["max_children"] = json!(16);
        stamped["max_depth"] = json!(6);
        let mut drifted = live.clone();
        drifted["max_validation_retries"] = json!(5);
        drifted["co_author"] = json!(false);
        drifted["provider_identity_prompt"] = json!("x");

        assert_eq!(configuration_problems(&live), Vec::<String>::new());
        assert_eq!(configuration_problems(&stamped), Vec::<String>::new());
        assert_eq!(configuration_problems(&json!({})), Vec::<String>::new());
        assert_eq!(
            configuration_problems(&drifted),
            [
                "retired key provider_identity_prompt is still configured",
                "max_validation_retries is 5",
                "co_author is false",
            ]
        );
    }
}
