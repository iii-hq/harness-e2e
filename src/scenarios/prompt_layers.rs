//! `prompt_layers` — the layers the Harness builds a system prompt from,
//! read back through `harness::system-prompt::get`. It changes stack-wide
//! defaults (the stored `default` prompt, the Harness
//! `default_filesystem_root`, the skill catalog) and restores them in
//! `cleanup`, so it needs a stack of its own: never a shared one.
//!
//! - The root's first exchange is in Portuguese: the runtime aid pins
//!   Portuguese on turn 0 and keeps it through an English second exchange.
//! - A run-scoped canary the root called in exchange 0 is re-registered
//!   with a new contract before exchange 1: exactly one `registry-changed`
//!   notice follows.
//! - While exchange 1 holds a call, the runner adds an offered skill and a
//!   `disable-model-invocation` one: the turn gets exactly one
//!   `e_<turn>_skills_*` correction naming the first only, and a later
//!   session's `<available_skills>` lists the first and not the second.
//! - A stored `default` prompt holding `STK-<hex>` becomes the default
//!   identity, removing it falls back to the embedded prompt (the directory
//!   refuses a blank body), and the live root keeps the prompt it was
//!   frozen with.
//! - A side session sent `system_prompt` with the `enrich` strategy runs on
//!   identity + "\n\n" + custom, keeps it on a send that names no prompt,
//!   and drops it on a send that names only the strategy.
//! - `configuration::set` of `default_filesystem_root` reaches
//!   `harness::filesystem::info` and a new session's working-directory aid
//!   without a restart.

use std::time::Duration;

use iii_sdk::runtime::FunctionRef;
use iii_sdk::{IIIClient, RegisterFunction};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common::lock_unpoisoned;
use super::mechanism::{self, call, record};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "prompt_layers";
const DELIVERABLE_ID: &str = "prompt_layers_checks";
const PREVIEW: &str = "harness::system-prompt::get";
const SKILLS_GET: &str = "directory::skills::get";
const STORED_DEFAULT: &str = "default";
const SKILLS_SETTLE: Duration = Duration::from_secs(3);
const TURN_WAIT: Duration = Duration::from_secs(120);

const LANGUAGE: AssessmentSpec = AssessmentSpec::gate(
    "language_pinned",
    15,
    "Did the runtime aid pin Portuguese from the first exchange and keep it after an English one?",
)
.labeled("The reply language is pinned on turn 0");
const REGISTRY: AssessmentSpec = AssessmentSpec::gate(
    "registry_changed",
    15,
    "After a function the session called was re-registered with a new contract, was there exactly one `registry-changed` notice?",
)
.labeled("One notice per re-registered function");
const SKILLS: AssessmentSpec = AssessmentSpec::gate(
    "skills_sync",
    20,
    "Did a skill added mid-turn reach that turn as exactly one skills correction, and a later session's index, while a `disable-model-invocation` skill stayed out of both?",
)
.labeled("New skills reach the index, disabled ones do not");
const STORED: AssessmentSpec = AssessmentSpec::gate(
    "stored_default",
    20,
    "Did a stored `default` prompt become the default identity, its removal fall back to the embedded prompt, and the live session keep its frozen prompt?",
)
.labeled("The stored default prompt applies and resets");
const ENRICH: AssessmentSpec = AssessmentSpec::gate(
    "sticky_enrich",
    15,
    "Did `enrich` compose identity + blank line + custom, stay on a send naming no prompt, and reset on a send naming only the strategy?",
)
.labeled("An enriched prompt is sticky until reset");
const FS_ROOT: AssessmentSpec = AssessmentSpec::gate(
    "fs_root_reload",
    15,
    "Did `configuration::set` of `default_filesystem_root` reach `harness::filesystem::info` and a new session's working-directory aid without a restart?",
)
.labeled("The default filesystem root hot-reloads");
const ASSESSMENTS: &[AssessmentSpec] = &[LANGUAGE, REGISTRY, SKILLS, STORED, ENRICH, FS_ROOT];

fn canary_function(run_id: &str) -> String {
    format!("e2etest::layers_canary_{}", mechanism::short(run_id))
}

fn hold_function(run_id: &str) -> String {
    format!("e2etest::layers_hold_{}", mechanism::short(run_id))
}

fn skill(run_id: &str, kind: &str) -> String {
    format!("e2e-layers-{}-{kind}", mechanism::short(run_id))
}

fn code(run_id: &str, label: &str) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("{label}-{hex:06x}")
}

fn side(run_id: &str, name: &str) -> String {
    format!("{}_{name}", mechanism::session_id(run_id))
}

fn harness_config_id(client: &IIIClient) -> String {
    format!(
        "{}-harness",
        client.namespace().as_deref().unwrap_or("default")
    )
}

fn prompts(run_id: &str) -> [String; 2] {
    let (canary, hold) = (canary_function(run_id), hold_function(run_id));
    [
        format!(
            "Chame `{canary}` uma vez com `{{}}`. Depois responda exatamente `OK {}` e nada mais.",
            code(run_id, "PL0")
        ),
        format!(
            "Call `{canary}` once with `{{}}`, then call `{hold}` once with `{{}}` and wait for it \
             to return. Then reply with exactly `OK {}` and nothing else.",
            code(run_id, "PL1")
        ),
    ]
}

/// The canary registrations, so the re-registration can drop the first.
fn canaries() -> &'static std::sync::Mutex<std::collections::HashMap<String, FunctionRef>> {
    static CANARIES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, FunctionRef>>,
    > = std::sync::OnceLock::new();
    CANARIES.get_or_init(Default::default)
}

fn register_canary(client: &IIIClient, run_id: &str, version: u32) {
    // The SDK holds one handler per id: drop the old contract first.
    if let Some(previous) = lock_unpoisoned(canaries()).remove(run_id) {
        previous.unregister();
    }
    let function = client.register_function(
        canary_function(run_id),
        RegisterFunction::new_async(move |_: Value| async move {
            Ok::<Value, iii_sdk::errors::Error>(json!({ "version": version }))
        })
        .description(format!(
            "E2E run-scoped canary, contract version {version}: returns its version."
        )),
    );
    lock_unpoisoned(canaries()).insert(run_id.to_string(), function);
}

pub struct PromptLayers;

#[async_trait]
impl Scenario for PromptLayers {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Build the system prompt from its layers")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Two short exchanges, one in Portuguese; around them the runner changes the stored default prompt, the skill catalog, a function's contract and the default filesystem root, and reads back the prompt each session gets.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("Operator defaults, caller prompts, skills, the reply language and the working directory reach every agent's system prompt as configured, change without a restart, and never rewrite a live session. If it fails, agents run with stale identities, skills or directories, answer in the wrong language, or keep stale tool contracts.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "stack": "dedicated: changes stack-wide defaults and restores them" }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::IiiFunctions,
            ],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let [prompt, _] = prompts(run_id);
        ScenarioSpec {
            id: ID,
            prompt,
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(6),
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(120_000),
                stuck_timeout_seconds: 240,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        prompts(run_id).into_iter().skip(1).collect()
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![canary_function(run_id), hold_function(run_id)]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        let mut allowed = self.required_functions(run_id);
        // A session gets a skills index only when it may load a skill.
        allowed.push(SKILLS_GET.to_string());
        Some(allowed)
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        for name in ["enrich", "fresh"] {
            mechanism::keep_session(run_id, &side(run_id, name));
        }
        // What cleanup restores, read before anything changes.
        let config = context
            .trigger_value(
                "configuration::get",
                json!({ "id": harness_config_id(context.client()) }),
            )
            .await?;
        mechanism::note(run_id, "harness_config", config["value"].clone());
        let stored = stored_default(context).await;
        mechanism::note(run_id, "stored_default", stored.unwrap_or(Value::Null));

        register_canary(context.client(), run_id, 1);
        let client = context.client().clone();
        let owner = run_id.to_string();
        let function = context.client().register_function(
            hold_function(run_id),
            RegisterFunction::new_async(move |_: Value| {
                let client = client.clone();
                let owner = owner.clone();
                async move {
                    add_skills(&client, &owner).await;
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "released": true }))
                }
            })
            .description("E2E run-scoped checkpoint: returns once the runner releases it."),
        );
        mechanism::keep_function(run_id, function);
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        _exchange: usize,
    ) {
        let runtime = preview_part(context, session_id, "runtime").await;
        record(
            run_id,
            "language_turn_0",
            runtime.contains("Response language: Portuguese"),
            excerpt(&runtime, "Response language"),
        );
        // A new contract for a function the session already called.
        register_canary(context.client(), run_id, 2);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let root = mechanism::session_id(run_id);
        let transcript = &observation.transcript;
        let runtime = preview_part(context, &root, "runtime").await;
        record(
            run_id,
            "language_kept",
            runtime.contains("Response language: Portuguese"),
            excerpt(&runtime, "Response language"),
        );
        let notices: Vec<&str> = mechanism::customs(transcript, "model_notice")
            .into_iter()
            .filter(|(_, data)| data["kind"] == "registry-changed")
            .map(|(id, _)| id)
            .collect();
        record(
            run_id,
            "registry_changed_once",
            notices.len() == 1,
            format!("registry-changed notices: {notices:?}"),
        );
        skills_correction(transcript, run_id);
        stored_default_checks(context, run_id).await;
        enrich_checks(context, observation, run_id).await;
        fs_root_checks(context, run_id).await;
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let calls = common::function_calls(&observation.transcript);
        let count = |function: String| {
            calls
                .iter()
                .filter(|call| call.function_id == function)
                .count()
        };
        if count(canary_function(run_id)) < 2 || count(hold_function(run_id)) == 0 {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "fixture_calls",
                "the subject did not call the canary in both exchanges and the hold",
            ));
        }
        let checks = mechanism::observed_checks(observation, run_id);
        Ok(assessment::build_evaluation([
            mechanism::grade(LANGUAGE, &checks, &["language_turn_0", "language_kept"]),
            mechanism::grade(REGISTRY, &checks, &["registry_changed_once"]),
            mechanism::grade(
                SKILLS,
                &checks,
                &["skills_added", "skills_correction_once", "skills_index"],
            ),
            mechanism::grade(
                STORED,
                &checks,
                &[
                    "stored_default_applies",
                    "stored_default_resets",
                    "live_prompt_frozen",
                ],
            ),
            mechanism::grade(
                ENRICH,
                &checks,
                &["enrich_composed", "enrich_sticky", "enrich_reset"],
            ),
            mechanism::grade(FS_ROOT, &checks, &["fs_root_info", "fs_root_aid"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        if let Some(config) =
            mechanism::noted(run_id, "harness_config").filter(|value| !value.is_null())
        {
            if let Err(error) = context
                .trigger_value(
                    "configuration::set",
                    json!({ "id": harness_config_id(context.client()), "value": config }),
                )
                .await
            {
                failures.push(format!("restore harness config: {error:#}"));
            }
        }
        if let Err(error) =
            restore_stored_default(context, mechanism::noted(run_id, "stored_default")).await
        {
            failures.push(format!("restore stored default: {error:#}"));
        }
        for kind in ["on", "off"] {
            let _ = context
                .trigger_value(
                    "directory::skills::delete",
                    json!({ "id": skill(run_id, kind) }),
                )
                .await;
        }
        if let Some(canary) = lock_unpoisoned(canaries()).remove(run_id) {
            canary.unregister();
        }
        mechanism::release(context, run_id).await?;
        anyhow::ensure!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }
}

fn excerpt(text: &str, around: &str) -> String {
    match text.find(around) {
        Some(at) => text[at..].chars().take(120).collect(),
        None => format!("no `{around}` in {} chars", text.len()),
    }
}

/// The body of the first `kind` part of `session_id`'s prompt preview.
async fn preview_part(context: &E2eContext, session_id: &str, kind: &str) -> String {
    preview(context, json!({ "session_id": session_id }))
        .await
        .and_then(|preview| part(&preview, kind))
        .unwrap_or_default()
}

async fn preview(context: &E2eContext, request: Value) -> Option<Value> {
    context.trigger_value(PREVIEW, request).await.ok()
}

fn part(preview: &Value, kind: &str) -> Option<String> {
    preview["parts"]
        .as_array()?
        .iter()
        .find(|part| part["kind"] == kind)
        .and_then(|part| part["body"].as_str().map(str::to_string))
}

/// The stored `default` system prompt's full content, when one exists.
async fn stored_default(context: &E2eContext) -> Option<Value> {
    let listed = context
        .trigger_value("directory::system-prompts::list", json!({}))
        .await
        .ok()?;
    let exists = listed["prompts"]
        .as_array()?
        .iter()
        .any(|prompt| prompt["name"] == STORED_DEFAULT && prompt["builtin"] != true);
    if !exists {
        return None;
    }
    context
        .trigger_value(
            "directory::system-prompts::get",
            json!({ "name": STORED_DEFAULT }),
        )
        .await
        .ok()
        .map(|prompt| prompt["content"].clone())
}

async fn restore_stored_default(
    context: &E2eContext,
    original: Option<Value>,
) -> anyhow::Result<()> {
    match original.filter(|content| content.is_string()) {
        Some(content) => {
            context
                .trigger_value(
                    "directory::system-prompts::update",
                    json!({ "name": STORED_DEFAULT, "content": content }),
                )
                .await?;
        }
        None => {
            if stored_default(context).await.is_some() {
                context
                    .trigger_value(
                        "directory::system-prompts::delete",
                        json!({ "name": STORED_DEFAULT }),
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

fn skill_content(run_id: &str, kind: &str, hidden: bool) -> String {
    let hide = if hidden {
        "disable-model-invocation: true\n"
    } else {
        ""
    };
    format!(
        "---\ntitle: E2E layers {kind} {}\ndescription: E2E prompt_layers skill ({kind}).\n{hide}---\n# E2E layers {kind}\n\nUse only when asked about prompt layers.\n",
        mechanism::short(run_id)
    )
}

/// Adds the offered and the hidden skill while the subject's call is held,
/// and lets the Harness catalog see them before the turn moves on.
async fn add_skills(client: &IIIClient, run_id: &str) {
    let mut outcomes = Vec::new();
    for (kind, hidden) in [("on", false), ("off", true)] {
        let created = call(
            client,
            "directory::skills::create",
            json!({ "id": skill(run_id, kind), "content": skill_content(run_id, kind, hidden) }),
        )
        .await;
        outcomes.push(format!(
            "{kind}: {}",
            created.map_or_else(|e| format!("{e:#}"), |_| "created".into())
        ));
    }
    tokio::time::sleep(SKILLS_SETTLE).await;
    record(
        run_id,
        "skills_added",
        outcomes.iter().all(|outcome| outcome.ends_with("created")),
        outcomes.join("; "),
    );
}

/// The skills correction the held turn got: one entry naming the offered
/// skill and not the hidden one.
fn skills_correction(transcript: &Value, run_id: &str) {
    let turns = mechanism::assistant_turns(transcript);
    let Some(turn) = turns.get(1) else {
        record(
            run_id,
            "skills_correction_once",
            false,
            format!("turns={turns:?}"),
        );
        return;
    };
    let prefix = format!("e_{turn}_skills_");
    let rows: Vec<String> = mechanism::entries(transcript)
        .filter(|entry| {
            entry["entry_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&prefix))
        })
        .map(Value::to_string)
        .collect();
    let (on, off) = (skill(run_id, "on"), skill(run_id, "off"));
    record(
        run_id,
        "skills_correction_once",
        rows.len() == 1 && rows[0].contains(&on) && !rows[0].contains(&off),
        format!(
            "{} `{prefix}*` entr(y/ies); names on={} off={}",
            rows.len(),
            rows.iter().any(|row| row.contains(&on)),
            rows.iter().any(|row| row.contains(&off))
        ),
    );
}

async fn stored_default_checks(context: &E2eContext, run_id: &str) {
    let marker = code(run_id, "STK");
    let fresh = side(run_id, "fresh");
    let content = format!("---\ndescription: E2E stored default identity\n---\n{marker} You are the E2E stored identity.\n");
    let written = match stored_default(context).await {
        Some(_) => {
            context
                .trigger_value(
                    "directory::system-prompts::update",
                    json!({ "name": STORED_DEFAULT, "content": content }),
                )
                .await
        }
        None => {
            context
                .trigger_value(
                    "directory::system-prompts::create",
                    json!({ "name": STORED_DEFAULT, "content": content }),
                )
                .await
        }
    };
    let default_only = || {
        preview(
            context,
            json!({ "session_id": fresh, "default_only": true }),
        )
    };
    let applied = default_only().await;
    let built_in = |preview: &Option<Value>| {
        preview
            .as_ref()
            .and_then(|preview| preview["parts"].as_array()?.first().cloned())
            .unwrap_or(Value::Null)
    };
    let applied = built_in(&applied);
    record(
        run_id,
        "stored_default_applies",
        written.is_ok()
            && applied["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("stored default"))
            && applied["body"]
                .as_str()
                .is_some_and(|body| body.contains(&marker)),
        format!(
            "write={:?} built_in name={}",
            written.as_ref().map(|_| "ok"),
            applied["name"]
        ),
    );
    let frozen = preview_part(context, &mechanism::session_id(run_id), "built_in").await;
    record(
        run_id,
        "live_prompt_frozen",
        !frozen.is_empty() && !frozen.contains(&marker),
        format!(
            "root built-in: {} chars, carries the stored marker={}",
            frozen.len(),
            frozen.contains(&marker)
        ),
    );
    let removed = context
        .trigger_value(
            "directory::system-prompts::delete",
            json!({ "name": STORED_DEFAULT }),
        )
        .await;
    let reset = built_in(&default_only().await);
    record(
        run_id,
        "stored_default_resets",
        reset["name"] == "embedded harness default"
            && reset["body"]
                .as_str()
                .is_some_and(|body| !body.contains(&marker)),
        format!(
            "delete={:?} built_in name={}",
            removed.as_ref().map(|_| "ok").map_err(|e| format!("{e:#}")),
            reset["name"]
        ),
    );
    if let Err(error) =
        restore_stored_default(context, mechanism::noted(run_id, "stored_default")).await
    {
        record(
            run_id,
            "stored_default_restored",
            false,
            format!("{error:#}"),
        );
    }
}

async fn wait_turn(context: &E2eContext, session_id: &str) -> Option<String> {
    mechanism::poll(TURN_WAIT, || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session_id }))
            .await
            .ok()?;
        matches!(
            status["status"].as_str(),
            Some("completed" | "failed" | "cancelled")
        )
        .then(|| status["status"].as_str().unwrap_or_default().to_string())
    })
    .await
}

async fn enrich_checks(context: &E2eContext, observation: &ScenarioObservation, run_id: &str) {
    let Some((model, provider)) = mechanism::subject_route(&observation.transcript) else {
        record(
            run_id,
            "enrich_composed",
            false,
            "no assistant reply to take the route from",
        );
        return;
    };
    let session = side(run_id, "enrich");
    let custom = code(run_id, "CUSTOM");
    let identity = preview(
        context,
        json!({ "session_id": session, "default_only": true }),
    )
    .await
    .and_then(|preview| part(&preview, "built_in"))
    .unwrap_or_default();
    let reply = |label: &str| format!("Reply with exactly `OK {label}` and nothing else.");
    let mut sends = vec![
        json!({
            "session_id": session, "message": reply("E1"), "model": model, "provider": provider,
            "session": { "kind": crate::wire::E2E_SESSION_KIND, "title": "Harness E2E: prompt_layers enrich" },
            "options": { "system_prompt": custom, "system_prompt_strategy": "enrich", "functions": { "allow": [SKILLS_GET], "deny": [] } },
        }),
        json!({ "session_id": session, "message": reply("E2") }),
        json!({ "session_id": session, "message": reply("E3"), "options": { "system_prompt_strategy": "enrich" } }),
    ]
    .into_iter();
    let expected = [
        ("enrich_composed", format!("{identity}\n\n{custom}")),
        ("enrich_sticky", format!("{identity}\n\n{custom}")),
        ("enrich_reset", identity.clone()),
    ];
    for (check, want) in expected {
        let send = sends.next().expect("one send per check");
        let sent = context.trigger_value("harness::send", send).await;
        let ended = match &sent {
            Ok(_) => wait_turn(context, &session).await,
            Err(_) => None,
        };
        let built_in = preview_part(context, &session, "built_in").await;
        record(
            run_id,
            check,
            !identity.is_empty() && built_in == want,
            format!(
                "send={:?} turn={ended:?} built_in {} chars, ends with custom={}",
                sent.as_ref().map(|_| "ok").map_err(|e| format!("{e:#}")),
                built_in.len(),
                built_in.ends_with(&custom)
            ),
        );
    }
    // The skills index a session started after the catalog change got.
    let skills = preview_part(context, &session, "skills").await;
    let (on, off) = (skill(run_id, "on"), skill(run_id, "off"));
    record(
        run_id,
        "skills_index",
        skills.contains("<available_skills>") && skills.contains(&on) && !skills.contains(&off),
        format!(
            "index lists on={} off={}",
            skills.contains(&on),
            skills.contains(&off)
        ),
    );
}

async fn fs_root_checks(context: &E2eContext, run_id: &str) {
    let Some(original) = mechanism::noted(run_id, "harness_config").filter(Value::is_object) else {
        record(
            run_id,
            "fs_root_info",
            false,
            "no Harness configuration to change",
        );
        return;
    };
    let root = std::env::temp_dir().join(format!("e2e-prompt-layers-{}", mechanism::short(run_id)));
    let _ = std::fs::create_dir_all(&root);
    let root = root.to_string_lossy().to_string();
    let mut changed = original.clone();
    changed["default_filesystem_root"] = json!(root);
    let set = context
        .trigger_value(
            "configuration::set",
            json!({ "id": harness_config_id(context.client()), "value": changed }),
        )
        .await;
    let seen = mechanism::poll(Duration::from_secs(30), || async {
        let info = context
            .trigger_value("harness::filesystem::info", json!({}))
            .await
            .ok()?;
        (info["default_root"] == root.as_str()).then_some(())
    })
    .await;
    record(
        run_id,
        "fs_root_info",
        set.is_ok() && seen.is_some(),
        format!(
            "set={:?} reflected={}",
            set.as_ref().map(|_| "ok").map_err(|e| format!("{e:#}")),
            seen.is_some()
        ),
    );
    let runtime = preview_part(context, &side(run_id, "fresh"), "runtime").await;
    record(
        run_id,
        "fs_root_aid",
        runtime.contains(&format!("Your working directory is {root}")),
        excerpt(&runtime, "Your working directory"),
    );
    let _ = context
        .trigger_value(
            "configuration::set",
            json!({ "id": harness_config_id(context.client()), "value": original }),
        )
        .await;
    let _ = std::fs::remove_dir(&root);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_skill_says_so_in_its_frontmatter() {
        assert!(
            skill_content("run", "off", true).contains("\ndisable-model-invocation: true\n---\n")
        );
        assert!(!skill_content("run", "on", false).contains("disable-model-invocation"));
    }

    #[test]
    fn a_preview_part_is_read_by_kind() {
        let preview = json!({ "parts": [
            { "kind": "built_in", "body": "identity" },
            { "kind": "runtime", "body": "Response language: Portuguese (from …)" },
        ] });
        assert_eq!(part(&preview, "built_in").as_deref(), Some("identity"));
        assert_eq!(part(&preview, "skills"), None);
        assert_eq!(
            excerpt("x Response language: Portuguese", "Response language"),
            "Response language: Portuguese"
        );
    }

    #[test]
    fn the_skills_correction_names_only_the_offered_skill() {
        let run = "skills-correction-test";
        let transcript = |rows: Vec<Value>| {
            let mut messages = vec![
                json!({ "entry_id": "e_t_a_0_assistant" }),
                json!({ "entry_id": "e_t_b_0_assistant" }),
            ];
            messages.extend(rows);
            json!({ "messages": messages })
        };
        let on = json!({ "entry_id": "e_t_b_skills_2", "message": { "role": "user", "content": [{ "type": "text", "text": skill(run, "on") }] } });
        skills_correction(&transcript(vec![on.clone()]), run);
        let both = json!({ "entry_id": "e_t_b_skills_2", "message": { "content": [{ "text": format!("{} {}", skill(run, "on"), skill(run, "off")) }] } });
        skills_correction(&transcript(vec![both]), run);
        skills_correction(
            &transcript(vec![on.clone(), json!({ "entry_id": "e_t_b_skills_3" })]),
            run,
        );
        let passed: Vec<bool> = mechanism::checks(run)
            .into_iter()
            .filter(|check| check.id == "skills_correction_once")
            .map(|check| check.passed)
            .collect();
        assert_eq!(passed, [true, false, false]);
        mechanism::reset(run);
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        PromptLayers.spec("run").validate().unwrap();
    }
}
