//! `dispatch_pipeline` — the Harness's function-dispatch pipeline, stage by
//! stage: policy, the `pre_trigger` chain, the target, result normalization,
//! the `post_trigger` chain, hook holds, and what discovery shows.
//!
//! The run registers its own fixtures (`pipe_*`) and hooks. The subject's
//! one turn calls `pipe_echo` and then `pipe_held`, which a hook holds; the
//! hook's own resolver unregisters `pipe_gone`, waits for the registry to
//! notice, and releases the call with `harness::function::resolve`, so the
//! resumed step must tell the model the registry changed. Every other probe
//! is a direct `harness::function::trigger` on the root after its turn,
//! which runs the same pipeline under the turn's recorded policy, plus one
//! spawned child that may call only `pipe_echo`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use iii_sdk::protocol::{RegisterTriggerInput, TriggerRequest};
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::{IIIClient, RegisterFunction};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::probe::{self, Answer, Probes};
use super::validation_loop::suffix;
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "dispatch_pipeline";
const DELIVERABLE_ID: &str = "dispatch_pipeline_probes";
const PRE_TRIGGER: &str = "harness::hook::pre-trigger";
const POST_TRIGGER: &str = "harness::hook::post-trigger";
/// The fixtures, by name; each is `e2etest::pipe_<name>_<suffix>`.
const FIXTURES: &[&str] = &[
    "echo", "held", "big", "err", "str", "content", "empty", "denied", "gone",
];
const DENIED_GLOB: &str = "e2etest::pipe_denied_*";
/// How long the resolver gives the Harness's registry snapshot to see
/// `pipe_gone` leave: older engines report a removal on a 5 s poll.
const REGISTRY_SETTLE: Duration = Duration::from_secs(6);

const DENIALS: AssessmentSpec = AssessmentSpec::gate(
    "denials",
    15,
    "Are a denied id, an unregistered id and a session with no turn record refused as policy_denied, with no pre_trigger hook consulted?",
)
.labeled("Policy refuses before any hook runs");
const NORMALIZATION: AssessmentSpec = AssessmentSpec::scored(
    "normalization",
    10,
    "Does a string become one text block, {content: []} the text {\"content\":[]}, is_error survive, and an oversized result get capped?",
)
.labeled("Results normalized and capped");
const HOOK_CHAIN: AssessmentSpec = AssessmentSpec::gate(
    "hook_chain",
    25,
    "Do pre_trigger hooks run in priority order, a deny and a throwing hook refuse the call as hook_denied, a timed-out fail_open hook let it through, a post_trigger rewrite land, and an invalid hook config get refused?",
)
.labeled("Hook chain: order, deny, fail-closed, fail-open, rewrite");
const HELD_CALL: AssessmentSpec = AssessmentSpec::gate_in(
    "held_call",
    25,
    "Does a held call run exactly once with the hook's arguments when resolved with execute, a second resolve report resolved:false, and a direct call to a held target only report pending?",
    EvaluationDimension::Deliverable,
)
.labeled("A held call runs once, as the hook changed it");
const DISCOVERY: AssessmentSpec = AssessmentSpec::scored(
    "discovery",
    25,
    "Does discovery hide denied ids, expose the functions through agent_trigger, narrow a spawned child to pipe_echo, and announce a registry change to the model?",
)
.labeled("Discovery shows only what may be called, and changes");
const ASSESSMENTS: &[AssessmentSpec] = &[DENIALS, NORMALIZATION, HOOK_CHAIN, HELD_CALL, DISCOVERY];

fn fixture_id(run_id: &str, name: &str) -> String {
    format!("e2etest::pipe_{name}_{}", suffix(run_id))
}

fn hook_id(run_id: &str, name: &str) -> String {
    format!("e2etest::pipe_hook_{name}_{}", suffix(run_id))
}

fn child_session(run_id: &str) -> String {
    format!("e2e_{run_id}_child")
}

/// What the fixtures and hooks saw, in order.
#[derive(Debug, Default)]
struct Ledger {
    /// Every fixture invocation: `{fixture, arguments}`.
    calls: Vec<Value>,
    /// What the pre_trigger observer bound to every fixture saw.
    observed: Vec<Value>,
    /// The resolver's two `harness::function::resolve` answers.
    resolves: Vec<Value>,
    /// The first hold already has its resolver; later holds only report.
    holding: bool,
}

impl Ledger {
    fn calls_to(&self, fixture: &str) -> Vec<Value> {
        self.calls
            .iter()
            .filter(|call| call["fixture"] == fixture)
            .map(|call| call["arguments"].clone())
            .collect()
    }
}

struct Runtime {
    functions: Vec<FunctionRef>,
    triggers: Vec<Trigger>,
    /// `pipe_gone`, until the resolver unregisters it.
    gone: Arc<Mutex<Option<FunctionRef>>>,
    ledger: Arc<Mutex<Ledger>>,
}

fn runtimes() -> &'static Mutex<HashMap<String, Runtime>> {
    static RUNTIMES: OnceLock<Mutex<HashMap<String, Runtime>>> = OnceLock::new();
    RUNTIMES.get_or_init(Default::default)
}

fn ledger(run_id: &str) -> Option<Arc<Mutex<Ledger>>> {
    common::lock_unpoisoned(runtimes())
        .get(run_id)
        .map(|runtime| Arc::clone(&runtime.ledger))
}

/// The arguments a fixture was called with, without the worker id the
/// engine stamps on every invocation.
fn caller_arguments(input: &Value) -> Value {
    let mut arguments = input.clone();
    if let Some(object) = arguments.as_object_mut() {
        object.retain(|key, _| !key.starts_with("_caller"));
    }
    arguments
}

/// What fixture `name` answers: each shape is one normalization case.
fn fixture_answer(name: &str, input: &Value) -> Value {
    match name {
        "echo" => json!({ "echo": input }),
        "big" => json!({ "blob": "x".repeat(300_000) }),
        "err" => json!({ "is_error": true, "content": [{ "type": "text", "text": "N2-ERROR" }] }),
        "str" => json!("N2-STRING"),
        "content" => json!({ "content": [{ "type": "text", "text": "N2-ORIGINAL" }] }),
        "empty" => json!({ "content": [] }),
        _ => json!({ "ran": true, "arguments": input }),
    }
}

/// The two chained pre_trigger hooks on `pipe_echo`. `z` runs first by
/// priority although it sorts last by id; it also denies or throws when
/// the call asks it to.
fn chain_answer(mark: &str, input: &Value) -> Result<Value, iii_sdk::errors::Error> {
    let mut arguments = input
        .pointer("/call/arguments")
        .cloned()
        .unwrap_or(json!({}));
    match (mark, arguments.get("mode").and_then(Value::as_str)) {
        ("z", Some("deny")) => return Ok(json!({ "decision": "deny", "reason": "N2-DENIED" })),
        ("z", Some("throw")) => return Err(iii_sdk::errors::Error::Handler("N2-THROWN".into())),
        _ => {}
    }
    if let Some(trail) = arguments.get_mut("trail").and_then(Value::as_array_mut) {
        trail.push(json!(mark));
    }
    Ok(json!({ "mutations": { "arguments": arguments } }))
}

pub struct DispatchPipeline;

#[async_trait]
impl Scenario for DispatchPipeline {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Every stage of a function call")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Call one fixture, then one a hook holds and changes; meanwhile the runner drives the dispatch pipeline directly through its policy, hooks, result normalization, holds and discovery.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("Every function call an agent makes passes the same pipeline: the policy refuses first, hooks run in order and fail as configured, results reach the model in one shape, an approval hold runs the call once as approved, and discovery shows only what may be called. If it fails, agents call what they must not, hooks are bypassed or run twice, or held calls are lost.")
    }

    fn characterization(&self) -> anyhow::Result<super::ScenarioCharacterization> {
        Ok(super::ScenarioCharacterization::synthetic())
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        FIXTURES
            .iter()
            .map(|name| fixture_id(run_id, name))
            .collect()
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        let mut allowed = self.required_functions(run_id);
        allowed.push("engine::functions::list".to_string());
        Some(allowed)
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "fixtures": FIXTURES,
                "denied": DENIED_GLOB,
                "held_arguments": { "note": "N2-CHANGED" },
                "chain": ["z", "a"],
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::IiiFunctions,
            ],
            probe::deliverable_contract(DELIVERABLE_ID, "harness_probes"),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let echo = fixture_id(run_id, "echo");
        let held = fixture_id(run_id, "held");
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Call `{echo}` with the arguments {{\"trail\": []}}. After it returns, call \
                 `{held}` with the arguments {{\"note\": \"original\"}}; it may take a few \
                 seconds. Make no other call. When both have returned, reply with exactly: DONE"
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(6),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(200_000),
                stuck_timeout_seconds: 240,
                max_validation_retries: None,
            },
            denied_functions: &[DENIED_GLOB],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release(run_id);
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let gone = Arc::new(Mutex::new(None));
        let client = context.client();
        let mut functions = Vec::new();
        for name in FIXTURES {
            let recorder = Arc::clone(&ledger);
            let function = client.register_function(
                fixture_id(run_id, name),
                RegisterFunction::new_async(move |input: Value| {
                    let recorder = Arc::clone(&recorder);
                    async move {
                        common::lock_unpoisoned(&recorder).calls.push(
                            json!({ "fixture": name, "arguments": caller_arguments(&input) }),
                        );
                        Ok::<Value, iii_sdk::errors::Error>(fixture_answer(name, &input))
                    }
                })
                // Public, unlike the hooks: discovery and the registry
                // snapshot behind the change notice only see public ids.
                .description("E2E dispatch_pipeline fixture."),
            );
            if *name == "gone" {
                *common::lock_unpoisoned(&gone) = Some(function);
            } else {
                functions.push(function);
            }
        }

        for mark in ["z", "a"] {
            functions.push(
                client.register_function(
                    hook_id(run_id, mark),
                    RegisterFunction::new_async(move |input: Value| async move {
                        chain_answer(mark, &input)
                    })
                    .description("E2E dispatch_pipeline chained pre_trigger hook.")
                    .metadata(json!({ "internal": true })),
                ),
            );
        }
        functions.push(
            client.register_function(
                hook_id(run_id, "slow"),
                RegisterFunction::new_async(|_input: Value| async {
                    // Answers long after its timeout; were it waited for, it denies.
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    Ok::<Value, iii_sdk::errors::Error>(
                        json!({ "decision": "deny", "reason": "late" }),
                    )
                })
                .description("E2E dispatch_pipeline slow fail_open hook.")
                .metadata(json!({ "internal": true })),
            ),
        );
        functions.push(
            client.register_function(
                hook_id(run_id, "post"),
                RegisterFunction::new_async(|_input: Value| async {
                    Ok::<Value, iii_sdk::errors::Error>(json!({
                        "mutations": { "content": [{ "type": "text", "text": "N2-REWRITTEN" }] },
                    }))
                })
                .description("E2E dispatch_pipeline post_trigger rewrite.")
                .metadata(json!({ "internal": true })),
            ),
        );
        functions.push(observer(
            client,
            hook_id(run_id, "observe"),
            &ledger,
            |input| {
                json!({
                    "session_id": input.get("session_id"),
                    "call_id": input.pointer("/call/id"),
                    "function_id": input.pointer("/call/function_id"),
                })
            },
        ));
        functions.push(holder(client, run_id, &ledger, &gone));

        let bind = |trigger_type: &str, hook: &str, config: Value| {
            client.register_trigger(RegisterTriggerInput::new(
                trigger_type,
                hook_id(run_id, hook),
                config,
            ))
        };
        let echo = fixture_id(run_id, "echo");
        let triggers = vec![
            bind(
                PRE_TRIGGER,
                "z",
                json!({ "functions": [echo], "priority": 10 }),
            )?,
            bind(
                PRE_TRIGGER,
                "a",
                json!({ "functions": [echo], "priority": 20 }),
            )?,
            bind(
                PRE_TRIGGER,
                "slow",
                json!({
                    "functions": [fixture_id(run_id, "str")],
                    "timeout_ms": 200, "on_error": "fail_open",
                }),
            )?,
            bind(
                PRE_TRIGGER,
                "hold",
                json!({ "functions": [fixture_id(run_id, "held")], "priority": 0 }),
            )?,
            bind(
                POST_TRIGGER,
                "post",
                json!({ "functions": [fixture_id(run_id, "content")] }),
            )?,
            // Bound to every id this run calls, the unregistered one included,
            // and to nothing else on the shared stack: last, quick, open on error.
            bind(
                PRE_TRIGGER,
                "observe",
                json!({
                    "functions": [format!("e2etest::pipe_*_{}", suffix(run_id))],
                    "priority": 1000, "timeout_ms": 2000, "on_error": "fail_open",
                }),
            )?,
        ];
        common::lock_unpoisoned(runtimes()).insert(
            run_id.to_string(),
            Runtime {
                functions,
                triggers,
                gone,
                ledger,
            },
        );
        // The engine hands the bindings to the Harness asynchronously.
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let root = observation.metrics.root_session_id.clone();
        let ledger = ledger(run_id).ok_or_else(|| anyhow::anyhow!("no fixtures for {run_id}"))?;
        let mut probes = Probes::default();
        let fixture = |name: &str| fixture_id(run_id, name);
        let call = |session: &str, id: &str, function_id: &str, arguments: Value| {
            probe::call(
                context,
                "harness::function::trigger",
                json!({
                    "session_id": session,
                    "call": { "id": id, "function_id": function_id, "arguments": arguments },
                }),
            )
        };

        // The resolver answers twice; give it time to finish the second.
        for _ in 0..60 {
            if common::lock_unpoisoned(&ledger).resolves.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let (resolves, held_calls) = {
            let ledger = common::lock_unpoisoned(&ledger);
            (ledger.resolves.clone(), ledger.calls_to("held"))
        };
        probes.check(
            "held.resolve_once",
            resolves.first().is_some_and(|first| {
                first.get("resolved") == Some(&json!(true))
                    && first.get("turn_resumed") == Some(&json!(true))
            }) && resolves
                .get(1)
                .is_some_and(|second| second.get("resolved") == Some(&json!(false))),
            &resolves,
        );
        probes.check(
            "held.ran_once_as_changed",
            held_calls == [json!({ "note": "N2-CHANGED" })],
            &held_calls,
        );
        let direct = call(
            &root,
            "n2-held",
            &fixture("held"),
            json!({ "note": "direct" }),
        )
        .await;
        let after = common::lock_unpoisoned(&ledger).calls_to("held").len();
        probes.check(
            "held.direct_reports_pending",
            direct
                .as_ref()
                .is_ok_and(|answer| answer.get("pending") == Some(&json!(true)))
                && after == held_calls.len(),
            json!({ "answer": direct, "held_calls": after }),
        );

        // Policy refuses before the hook chain.
        let denied = call(&root, "n2-denied", &fixture("denied"), json!({})).await;
        probes.check("deny.denied_id", is_policy_denied(&denied), &denied);
        let missing = call(&root, "n2-missing", &fixture("missing"), json!({})).await;
        probes.check("deny.unregistered_id", is_policy_denied(&missing), &missing);
        let recordless = call(
            &format!("{root}_no_record"),
            "n2-no-record",
            &fixture("echo"),
            json!({ "trail": [] }),
        )
        .await;
        probes.check(
            "deny.session_without_record",
            is_policy_denied(&recordless),
            &recordless,
        );

        // The hook chain.
        let order = call(&root, "n2-order", &fixture("echo"), json!({ "trail": [] })).await;
        probes.check(
            "hook.priority_order",
            details(&order).pointer("/echo/trail") == Some(&json!(["z", "a"])),
            &order,
        );
        let echoes = common::lock_unpoisoned(&ledger).calls_to("echo").len();
        let deny = call(
            &root,
            "n2-deny",
            &fixture("echo"),
            json!({ "mode": "deny", "trail": [] }),
        )
        .await;
        let throw = call(
            &root,
            "n2-throw",
            &fixture("echo"),
            json!({ "mode": "throw", "trail": [] }),
        )
        .await;
        let reached = common::lock_unpoisoned(&ledger).calls_to("echo").len() - echoes;
        probes.check(
            "hook.deny",
            hook_denied(&deny) && details(&deny)["message"] == "N2-DENIED" && reached == 0,
            json!({ "answer": deny, "echo_calls_after": reached }),
        );
        probes.check(
            "hook.throw_fails_closed",
            hook_denied(&throw) && reached == 0,
            json!({ "answer": throw, "echo_calls_after": reached }),
        );
        let late = call(&root, "n2-late", &fixture("str"), json!({})).await;
        probes.check(
            "hook.timeout_fails_open",
            !is_error(&late) && text(&late) == Some("N2-STRING"),
            &late,
        );
        let rewritten = call(&root, "n2-rewrite", &fixture("content"), json!({})).await;
        probes.check(
            "hook.post_rewrite",
            text(&rewritten) == Some("N2-REWRITTEN")
                && common::lock_unpoisoned(&ledger).calls_to("content").len() == 1,
            &rewritten,
        );
        let invalid = invalid_hook_config(context, run_id).await;
        probes.check("hook.invalid_config_refused", invalid.is_err(), &invalid);

        // Normalization.
        probes.check(
            "normalize.string",
            text(&late) == Some("N2-STRING") && blocks(&late) == 1,
            &late,
        );
        let empty = call(&root, "n2-empty", &fixture("empty"), json!({})).await;
        probes.check(
            "normalize.empty_content",
            text(&empty) == Some(r#"{"content":[]}"#) && !is_error(&empty),
            &empty,
        );
        let error = call(&root, "n2-err", &fixture("err"), json!({})).await;
        probes.check(
            "normalize.is_error_kept",
            is_error(&error) && text(&error) == Some("N2-ERROR"),
            &error,
        );
        let big = call(&root, "n2-big", &fixture("big"), json!({})).await;
        probes.check(
            "normalize.oversized_capped",
            text(&big).is_some_and(|text| text.starts_with("<omitted:"))
                && details(&big).get("result_capped").is_some(),
            json!({ "content": big.as_ref().ok().and_then(|answer| answer.get("content")), "details": details(&big) }),
        );

        // Discovery.
        let listed = call(
            &root,
            "n2-list",
            "engine::functions::list",
            json!({ "prefix": "e2etest::pipe_" }),
        )
        .await;
        let ids: Vec<&str> = details(&listed)
            .get("functions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|function| function.get("function_id").and_then(Value::as_str))
            .collect();
        let allowed = self.allowed_functions(run_id).unwrap_or_default();
        probes.check(
            "discovery.post_filter",
            ids.contains(&fixture("echo").as_str())
                && !ids.contains(&fixture("denied").as_str())
                && ids
                    .iter()
                    .all(|id| allowed.iter().any(|allowed| allowed == id)),
            &ids,
        );
        let (narrowed, seen) = spawned_child_policy(context, observation, run_id).await;
        probes.check("discovery.spawned_child_policy", narrowed, seen);
        let exposed = exposed_tools(&observation.transcript);
        probes.check(
            "discovery.agent_trigger_exposure",
            !exposed.is_empty() && exposed.iter().all(|tool| tool == "agent_trigger"),
            &exposed,
        );
        probes.check(
            "discovery.registry_changed_notice",
            registry_changed_notice(&observation.transcript),
            Value::Null,
        );

        let observed = common::lock_unpoisoned(&ledger).observed.clone();
        let saw = |call_id: &str| observed.iter().any(|seen| seen["call_id"] == call_id);
        probes.check(
            "deny.no_hook_consulted",
            saw("n2-order")
                && !["n2-denied", "n2-missing", "n2-no-record"]
                    .iter()
                    .any(|id| saw(id)),
            &observed,
        );

        Ok(vec![probes.deliverable(
            DELIVERABLE_ID,
            "harness_probes",
            &root,
        )])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        let Some(probes) = probe::captured(observation, DELIVERABLE_ID) else {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "probes_captured",
                "the capture recorded no probes",
            ));
        };
        Ok(assessment::build_evaluation([
            probe::award(
                DENIALS,
                &probes,
                &[
                    "deny.denied_id",
                    "deny.unregistered_id",
                    "deny.session_without_record",
                    "deny.no_hook_consulted",
                ],
            ),
            probe::award(
                NORMALIZATION,
                &probes,
                &[
                    "normalize.string",
                    "normalize.empty_content",
                    "normalize.is_error_kept",
                    "normalize.oversized_capped",
                ],
            ),
            probe::award(
                HOOK_CHAIN,
                &probes,
                &[
                    "hook.priority_order",
                    "hook.deny",
                    "hook.throw_fails_closed",
                    "hook.timeout_fails_open",
                    "hook.post_rewrite",
                    "hook.invalid_config_refused",
                ],
            ),
            probe::award(
                HELD_CALL,
                &probes,
                &[
                    "held.resolve_once",
                    "held.ran_once_as_changed",
                    "held.direct_reports_pending",
                ],
            ),
            probe::award(
                DISCOVERY,
                &probes,
                &[
                    "discovery.post_filter",
                    "discovery.spawned_child_policy",
                    "discovery.agent_trigger_exposure",
                    "discovery.registry_changed_notice",
                ],
            ),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release(run_id);
        let _ = context.teardown(&child_session(run_id)).await;
        Ok(())
    }
}

/// Unregisters every binding and fixture of the run; a no-op once done.
fn release(run_id: &str) {
    if let Some(runtime) = common::lock_unpoisoned(runtimes()).remove(run_id) {
        for trigger in runtime.triggers {
            trigger.unregister();
        }
        for function in runtime.functions {
            function.unregister();
        }
        if let Some(gone) = common::lock_unpoisoned(&runtime.gone).take() {
            gone.unregister();
        }
    }
}

/// A hook that records what `extract` reads of each call it is consulted
/// on, and never changes or stops anything.
fn observer(
    client: &IIIClient,
    function_id: String,
    ledger: &Arc<Mutex<Ledger>>,
    extract: fn(&Value) -> Value,
) -> FunctionRef {
    let ledger = Arc::clone(ledger);
    client.register_function(
        function_id,
        RegisterFunction::new_async(move |input: Value| {
            let ledger = Arc::clone(&ledger);
            async move {
                common::lock_unpoisoned(&ledger)
                    .observed
                    .push(extract(&input));
                Ok::<Value, iii_sdk::errors::Error>(Value::Null)
            }
        })
        .description("E2E dispatch_pipeline observer hook.")
        .metadata(json!({ "internal": true })),
    )
}

/// The hook that holds `pipe_held` with changed arguments. The first hold
/// in a turn gets a resolver: once the call shows as pending it removes
/// `pipe_gone`, lets the registry notice, then resolves the call twice.
fn holder(
    client: &IIIClient,
    run_id: &str,
    ledger: &Arc<Mutex<Ledger>>,
    gone: &Arc<Mutex<Option<FunctionRef>>>,
) -> FunctionRef {
    let resolver = client.clone();
    let ledger = Arc::clone(ledger);
    let gone = Arc::clone(gone);
    client.register_function(
        hook_id(run_id, "hold"),
        RegisterFunction::new_async(move |input: Value| {
            let resolver = resolver.clone();
            let ledger = Arc::clone(&ledger);
            let gone = Arc::clone(&gone);
            async move {
                let first = !std::mem::replace(&mut common::lock_unpoisoned(&ledger).holding, true);
                let session_id = input
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let turn_id = input
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let call_id = input
                    .pointer("/call/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let (true, Some(session_id), Some(turn_id), Some(call_id)) =
                    (first, session_id, turn_id, call_id)
                {
                    tokio::spawn(resolve_held(
                        resolver, ledger, gone, session_id, turn_id, call_id,
                    ));
                }
                Ok::<Value, iii_sdk::errors::Error>(json!({
                    "decision": "hold",
                    "mutations": { "arguments": { "note": "N2-CHANGED" } },
                }))
            }
        })
        .description("E2E dispatch_pipeline holding pre_trigger hook.")
        .metadata(json!({ "internal": true })),
    )
}

async fn resolve_held(
    client: IIIClient,
    ledger: Arc<Mutex<Ledger>>,
    gone: Arc<Mutex<Option<FunctionRef>>>,
    session_id: String,
    turn_id: String,
    call_id: String,
) {
    let invoke = |function_id: &str, payload: Value| {
        client.trigger(TriggerRequest {
            function_id: function_id.to_string(),
            payload,
            action: None,
            timeout_ms: Some(30_000),
        })
    };
    for _ in 0..120 {
        let pending = invoke("harness::status", json!({ "session_id": session_id }))
            .await
            .ok()
            .and_then(|status| status.get("pending_function_calls").cloned())
            .is_some_and(|pending| {
                pending
                    .as_array()
                    .is_some_and(|ids| ids.contains(&json!(call_id)))
            });
        if pending {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if let Some(function) = common::lock_unpoisoned(&gone).take() {
        function.unregister();
    }
    tokio::time::sleep(REGISTRY_SETTLE).await;
    let request = json!({
        "session_id": session_id, "turn_id": turn_id,
        "function_call_id": call_id, "action": "execute",
    });
    for _ in 0..2 {
        let answer = invoke("harness::function::resolve", request.clone())
            .await
            .unwrap_or_else(|error| json!({ "error": error.to_string() }));
        common::lock_unpoisoned(&ledger).resolves.push(answer);
    }
}

/// Registers a hook whose config the Harness must refuse; scoped to a
/// function that does not exist and removed again, should it be accepted.
async fn invalid_hook_config(context: &E2eContext, run_id: &str) -> Answer {
    let answer = probe::call(
        context,
        "engine::register_trigger",
        json!({
            "trigger_type": PRE_TRIGGER,
            "function_id": hook_id(run_id, "z"),
            "config": { "functions": [fixture_id(run_id, "none")], "on_error": "maybe" },
        }),
    )
    .await;
    if let Some(id) = answer
        .as_ref()
        .ok()
        .and_then(|registered| registered.get("id"))
    {
        let _ = probe::call(context, "engine::unregister_trigger", json!({ "id": id })).await;
    }
    answer
}

/// Spawns a child that may call only `pipe_echo` and checks, once its turn
/// settled, that it can call that and nothing else; returns the verdict and
/// what it saw.
async fn spawned_child_policy(
    context: &E2eContext,
    observation: &ScenarioObservation,
    run_id: &str,
) -> (bool, Value) {
    let Some((model, provider)) = probe::subject_route(&observation.transcript) else {
        return (false, json!("the root transcript names no model"));
    };
    let child = child_session(run_id);
    let spawned = probe::call(
        context,
        "harness::spawn",
        json!({
            "task": "Reply with exactly: OK",
            "model": model, "provider": provider, "session_id": child,
            "options": {
                "max_turns": 1, "max_output_tokens": 512,
                "functions": { "allow": [fixture_id(run_id, "echo")] },
            },
        }),
    )
    .await;
    let Some(turn_id) = spawned
        .as_ref()
        .ok()
        .and_then(|answer| answer.get("child_turn_id"))
        .and_then(Value::as_str)
    else {
        return (false, json!({ "spawn": spawned }));
    };
    if let Err(error) =
        probe::settled_status(context, &child, Some(turn_id), Duration::from_secs(90)).await
    {
        return (
            false,
            json!({ "spawn": spawned, "settle": format!("{error:#}") }),
        );
    }
    let call = |id: &str, function_id: String, arguments: Value| {
        probe::call(
            context,
            "harness::function::trigger",
            json!({ "session_id": child, "call": { "id": id, "function_id": function_id, "arguments": arguments } }),
        )
    };
    let echo = call(
        "n2-child-echo",
        fixture_id(run_id, "echo"),
        json!({ "trail": [] }),
    )
    .await;
    let other = call("n2-child-str", fixture_id(run_id, "str"), json!({})).await;
    (
        !is_error(&echo) && details(&echo).get("echo").is_some() && is_policy_denied(&other),
        json!({ "spawn": spawned, "echo": echo, "other": other }),
    )
}

fn details(answer: &Answer) -> &Value {
    answer
        .as_ref()
        .ok()
        .and_then(|answer| answer.get("details"))
        .unwrap_or(&Value::Null)
}

fn is_error(answer: &Answer) -> bool {
    answer
        .as_ref()
        .map_or(true, |answer| answer.get("is_error") == Some(&json!(true)))
}

fn text(answer: &Answer) -> Option<&str> {
    answer.as_ref().ok()?.pointer("/content/0/text")?.as_str()
}

fn blocks(answer: &Answer) -> usize {
    answer
        .as_ref()
        .ok()
        .and_then(|answer| answer.get("content"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn is_policy_denied(answer: &Answer) -> bool {
    is_error(answer) && details(answer)["error"] == "policy_denied"
}

fn hook_denied(answer: &Answer) -> bool {
    is_error(answer) && details(answer)["error"] == "hook_denied"
}

/// The tool names the subject's calls went out under, as the transcript
/// records them: `agent_trigger` wraps every call when that is the exposure.
fn exposed_tools(transcript: &Value) -> Vec<String> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("function_call"))
        .filter_map(|block| block.get("function_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn registry_changed_notice(transcript: &Value) -> bool {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|entry| {
            entry.pointer("/custom/custom_type") == Some(&json!("model_notice"))
                && entry.pointer("/custom/data/kind") == Some(&json!("registry-changed"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::probe::Probe;
    use crate::scenarios::ScenarioId;

    #[test]
    fn the_chain_marks_in_order_and_z_denies_or_throws_on_request() {
        let input = |arguments: Value| json!({ "call": { "arguments": arguments } });
        let z = chain_answer("z", &input(json!({ "trail": [] }))).unwrap();
        assert_eq!(z.pointer("/mutations/arguments/trail"), Some(&json!(["z"])));
        let denied = chain_answer("z", &input(json!({ "mode": "deny" }))).unwrap();
        assert_eq!(denied["decision"], "deny");
        assert!(chain_answer("z", &input(json!({ "mode": "throw" }))).is_err());
        let a = chain_answer("a", &input(json!({ "mode": "deny", "trail": ["z"] }))).unwrap();
        assert_eq!(
            a.pointer("/mutations/arguments/trail"),
            Some(&json!(["z", "a"]))
        );
    }

    #[test]
    fn answers_are_read_the_way_the_pipeline_returns_them() {
        let denied: Answer = Ok(json!({
            "is_error": true, "content": [{ "type": "text", "text": "no" }],
            "details": { "error": "policy_denied" },
        }));
        assert!(is_policy_denied(&denied));
        assert!(!hook_denied(&denied));
        assert!(is_error(&Err("down".into())));
        assert!(!is_policy_denied(&Err("down".into())));
        assert_eq!(text(&denied), Some("no"));
        assert_eq!(blocks(&denied), 1);
    }

    #[test]
    fn discovery_reads_the_wrapped_calls_and_the_notice() {
        let transcript = json!({ "messages": [
            { "message": { "role": "assistant", "content": [
                { "type": "text", "text": "x" },
                { "type": "function_call", "id": "c", "function_id": "agent_trigger",
                  "arguments": { "function": "e2etest::pipe_echo_x", "payload": {} } },
            ] } },
            { "custom": { "custom_type": "model_notice", "data": { "kind": "registry-changed" } } },
        ] });
        assert_eq!(exposed_tools(&transcript), ["agent_trigger"]);
        assert!(registry_changed_notice(&transcript));
        assert!(!registry_changed_notice(&json!({ "messages": [] })));
        assert!(exposed_tools(&json!({ "messages": [] })).is_empty());
    }

    #[tokio::test]
    async fn a_held_call_failing_leaves_the_task_incomplete() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let ids = [
            "deny.denied_id",
            "deny.unregistered_id",
            "deny.session_without_record",
            "deny.no_hook_consulted",
            "normalize.string",
            "normalize.empty_content",
            "normalize.is_error_kept",
            "normalize.oversized_capped",
            "hook.priority_order",
            "hook.deny",
            "hook.throw_fails_closed",
            "hook.timeout_fails_open",
            "hook.post_rewrite",
            "hook.invalid_config_refused",
            "held.resolve_once",
            "held.ran_once_as_changed",
            "held.direct_reports_pending",
            "discovery.post_filter",
            "discovery.spawned_child_policy",
            "discovery.agent_trigger_exposure",
            "discovery.registry_changed_notice",
        ];
        let evaluate = |failing: &'static str| {
            let probes = ids
                .iter()
                .map(|id| Probe {
                    id: (*id).into(),
                    passed: *id != failing,
                    observed: Value::Null,
                })
                .collect();
            let observation = ScenarioObservation::for_test(
                ScenarioId::DispatchPipeline,
                "control",
                json!({ "messages": [] }),
                "DONE",
                vec![Probes(probes).deliverable(DELIVERABLE_ID, "harness_probes", "e2e_control")],
            );
            async move {
                DispatchPipeline
                    .evaluate(&context, &observation, "control")
                    .await
                    .unwrap()
            }
        };
        let all = evaluate("none").await;
        let points: u16 = all
            .awards
            .iter()
            .filter_map(|award| award.awarded)
            .map(u16::from)
            .sum();
        assert_eq!(points, 100);
        let held = evaluate("held.ran_once_as_changed").await;
        assert_eq!(
            assessment::completion(ASSESSMENTS, &held),
            crate::report::CompletionState::TaskIncomplete
        );
        let notice = evaluate("discovery.registry_changed_notice").await;
        assert_eq!(
            assessment::completion(ASSESSMENTS, &notice),
            crate::report::CompletionState::Completed
        );
    }
}
