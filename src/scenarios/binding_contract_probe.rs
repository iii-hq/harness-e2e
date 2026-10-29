//! `binding_contract_probe` — trigger bindings, from registration to
//! teardown. After the subject's first answer, the runner registers bindings
//! for the root session the way an agent's own call would reach the Harness
//! (`harness::function::trigger` with `engine::register_trigger`), fires them
//! by writing the state keys they watch, and reads what each fire did:
//!
//! - registrations the Harness must refuse, each with its pinned error;
//! - call bindings: a `max_fires` budget, a burst, an expiry, and the
//!   session's own `fs_scope` stamped over a forged one;
//! - wakes: a burst, an expiry, an event cut to 8,000 characters, and one
//!   removed from outside the session;
//! - the cap of 64 bindings per session, filled with call bindings (a wake
//!   would cost a turn each), and `harness::teardown` clearing them.
//!
//! The wakes start turns of their own, so all of it runs before the bare
//! follow-up and the run's usage counts them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use iii_sdk::runtime::FunctionRef;
use iii_sdk::RegisterFunction;
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

pub const ID: &str = "binding_contract_probe";
const DELIVERABLE_ID: &str = "binding_contract_probes";
const PROMPT: &str = "Reply with exactly: OK. Every later message in this conversation is a \
notification sent by this test: answer each with exactly OK, and call no function.";
const CAP: usize = 64;
const WAKE_TEXT_MAX: usize = 8_000;
/// The state keys the fired bindings watch.
const KEYS: &[&str] = &[
    "max", "burst", "expiry", "scoped", "wburst", "wbig", "wgone",
];

const REFUSALS: AssessmentSpec = AssessmentSpec::gate(
    "refusals",
    20,
    "Are a string config, a harness trigger type, harness::spawn, an unpermitted or a nested-registration target, a zero max_fires, a non-positive or absolute deadline, an unknown call-metadata key, a relative event_into and a config condition all refused with their pinned errors?",
)
.labeled("Malformed and unsafe bindings refused");
const CALL_BINDINGS: AssessmentSpec = AssessmentSpec::gate_in(
    "call_bindings",
    30,
    "Does a call binding deliver exactly max_fires times, every event of a burst once, nothing after its deadline, and the session's fs_scope over a forged one?",
    EvaluationDimension::Deliverable,
)
.labeled("Call bindings fire exactly as bounded");
const WAKES: AssessmentSpec = AssessmentSpec::gate_in(
    "wakes",
    30,
    "Does a wake notify once per burst event, cut a long event to 8,000 characters, tell the session when it expires unfired, and when it is removed from outside, and never fire after removal?",
    EvaluationDimension::Deliverable,
)
.labeled("Wakes notify once, bounded, and on ending");
const CAPACITY: AssessmentSpec = AssessmentSpec::scored(
    "capacity",
    20,
    "Is the 65th binding of a session refused at the cap of 64, and does harness::teardown remove every binding left?",
)
.labeled("64 bindings per session, all torn down");
const ASSESSMENTS: &[AssessmentSpec] = &[REFUSALS, CALL_BINDINGS, WAKES, CAPACITY];

fn sink_id(run_id: &str) -> String {
    format!("e2etest::bind_sink_{}", suffix(run_id))
}

/// A sink the Harness stamps the session's `fs_scope` onto: it scopes every
/// `coder::` function.
fn scoped_sink_id(run_id: &str) -> String {
    format!("coder::e2etest_bind_scoped_{}", suffix(run_id))
}

fn scope(run_id: &str) -> String {
    format!("e2e-binding-{run_id}")
}

fn workspace(run_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("harness-e2e-binding-{run_id}"))
}

struct Run {
    functions: Vec<FunctionRef>,
    /// Every sink delivery: the payload as the target received it.
    deliveries: Arc<Mutex<Vec<Value>>>,
    probes: Vec<probe::Probe>,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(Default::default)
}

pub struct BindingContractProbe;

#[async_trait]
impl Scenario for BindingContractProbe {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Trigger bindings, from registration to teardown")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Answer OK, then each notification with OK, while the runner registers, fires, expires and removes the session's trigger bindings and fills its cap.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("An agent's trigger bindings fire exactly as bounded (count, deadline, burst), call their target under the session's own filesystem scope, wake the session once per event with a bounded message, tell it when a wake ends unfired, and are refused when malformed or past the cap. If it fails, agents miss events, act twice, act outside their workspace, or wait forever.")
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![sink_id(run_id), scoped_sink_id(run_id)]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        let mut allowed = self.required_functions(run_id);
        allowed.extend([
            "engine::register_trigger".to_string(),
            "engine::unregister_trigger".to_string(),
        ]);
        Some(allowed)
    }

    fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
        vec!["Reply with exactly: OK".to_string()]
    }

    fn bare_followup(&self, _followup: usize) -> bool {
        true
    }

    fn prepared_root(&self, run_id: &str) -> anyhow::Result<Option<PathBuf>> {
        Ok(Some(workspace(run_id)))
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "trigger_type": "state",
                "keys": KEYS,
                "binding_cap": CAP,
                "wake_text_max": WAKE_TEXT_MAX,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::IiiState,
                Capability::IiiTriggers,
            ],
            probe::deliverable_contract(DELIVERABLE_ID, "harness_probes"),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: PROMPT.to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(4),
                max_output_tokens: Some(1_024),
                // Unmetered: the bare follow-up names no output cap, so its
                // reservation is the model's whole output allocation.
                max_total_tokens: None,
                stuck_timeout_seconds: 300,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release(run_id);
        std::fs::create_dir_all(workspace(run_id))?;
        let deliveries = Arc::new(Mutex::new(Vec::new()));
        let functions = [sink_id(run_id), scoped_sink_id(run_id)]
            .into_iter()
            .map(|function_id| {
                let deliveries = Arc::clone(&deliveries);
                context.client().register_function(
                    function_id,
                    RegisterFunction::new_async(move |input: Value| {
                        let deliveries = Arc::clone(&deliveries);
                        async move {
                            common::lock_unpoisoned(&deliveries).push(input);
                            Ok::<Value, iii_sdk::errors::Error>(json!({ "received": true }))
                        }
                    })
                    .description("E2E binding_contract_probe sink: records each delivery.")
                    .metadata(json!({ "internal": true })),
                )
            })
            .collect();
        common::lock_unpoisoned(runs()).insert(
            run_id.to_string(),
            Run {
                functions,
                deliveries,
                probes: Vec::new(),
            },
        );
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        _exchange: usize,
    ) {
        let Some(deliveries) = common::lock_unpoisoned(runs())
            .get(run_id)
            .map(|run| Arc::clone(&run.deliveries))
        else {
            return;
        };
        let probe = Prober {
            context,
            run_id,
            root: session_id,
            deliveries,
        };
        let mut probes = Probes::default();
        probe.refusals(&mut probes).await;
        probe.call_bindings(&mut probes).await;
        probe.wakes(&mut probes).await;
        probe.capacity(&mut probes).await;
        if let Some(run) = common::lock_unpoisoned(runs()).get_mut(run_id) {
            run.probes = probes.0;
        }
    }

    async fn capture(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let probes = common::lock_unpoisoned(runs())
            .get_mut(run_id)
            .map(|run| std::mem::take(&mut run.probes))
            .unwrap_or_default();
        Ok(vec![Probes(probes).deliverable(
            DELIVERABLE_ID,
            "harness_probes",
            &observation.metrics.root_session_id,
        )])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        let probes = match probe::captured(observation, DELIVERABLE_ID) {
            Some(probes) if !probes.is_empty() => probes,
            _ => {
                return Ok(assessment::prerequisite_failure(
                    ASSESSMENTS,
                    "probes_captured",
                    "the bindings were never probed",
                ))
            }
        };
        Ok(assessment::build_evaluation([
            probe::award(REFUSALS, &probes, REFUSAL_PROBES),
            probe::award(
                CALL_BINDINGS,
                &probes,
                &[
                    "call.max_fires",
                    "call.burst",
                    "call.expiry",
                    "call.fs_scope",
                ],
            ),
            probe::award(
                WAKES,
                &probes,
                &[
                    "wake.burst",
                    "wake.truncated",
                    "wake.expiry",
                    "wake.external_unregister",
                ],
            ),
            probe::award(CAPACITY, &probes, &["capacity.cap", "capacity.teardown"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release(run_id);
        for key in KEYS {
            let _ = probe::call(
                context,
                "state::delete",
                json!({ "scope": scope(run_id), "key": key }),
            )
            .await;
        }
        let _ = std::fs::remove_dir_all(workspace(run_id));
        Ok(())
    }
}

fn release(run_id: &str) {
    if let Some(run) = common::lock_unpoisoned(runs()).remove(run_id) {
        for function in run.functions {
            function.unregister();
        }
    }
}

/// Each refusal: the probe id, the registration, and the text its error
/// must carry.
fn refusal_cases(run_id: &str) -> Vec<(&'static str, Value, &'static str)> {
    let sink = sink_id(run_id);
    let watch = json!({ "scope": scope(run_id), "key": "refused" });
    vec![
        (
            "refuse.config_string",
            json!({ "trigger_type": "state", "config": watch.to_string() }),
            "`config` must be a JSON object",
        ),
        (
            "refuse.harness_type",
            json!({ "trigger_type": "harness::turn-completed", "config": {} }),
            "cannot bind harness-internal trigger type",
        ),
        (
            "refuse.spawn_target",
            json!({ "trigger_type": "state", "config": watch, "function_id": "harness::spawn" }),
            "is not a binding target",
        ),
        (
            "refuse.unpermitted_target",
            json!({ "trigger_type": "state", "config": watch, "function_id": "state::set" }),
            "is not permitted by this session's dispatch policy",
        ),
        (
            "refuse.nested_registration",
            json!({
                "trigger_type": "state", "config": watch,
                "function_id": "engine::register_trigger",
            }),
            "requires the harness's agent-call intercept",
        ),
        (
            "refuse.zero_max_fires",
            json!({
                "trigger_type": "state", "config": watch, "function_id": sink,
                "lifecycle": { "max_fires": 0 },
            }),
            "`lifecycle.max_fires` must be at least 1",
        ),
        (
            "refuse.non_positive_deadline",
            json!({
                "trigger_type": "state", "config": watch, "function_id": sink,
                "lifecycle": { "expires_in_ms": -5 },
            }),
            "`lifecycle.expires_in_ms` must be positive",
        ),
        (
            "refuse.absolute_deadline",
            json!({
                "trigger_type": "state", "config": watch, "function_id": sink,
                "lifecycle": { "expires_at": 4_102_444_800_000_i64 },
            }),
            "unknown field `expires_at`",
        ),
        (
            "refuse.unknown_metadata_key",
            json!({
                "trigger_type": "state", "config": watch, "function_id": sink,
                "metadata": { "task": "spawn something" },
            }),
            "unknown key `task`",
        ),
        (
            "refuse.relative_event_into",
            json!({
                "trigger_type": "state", "config": watch,
                "target": { "function_id": sink, "event_into": "event" },
            }),
            "`event_into` must be a JSON pointer",
        ),
        (
            "refuse.config_condition",
            json!({
                "trigger_type": "state",
                "config": { "scope": scope(run_id), "key": "refused", "condition_function_id": sink },
                "function_id": sink,
            }),
            "`condition_function_id` in `config` is not supported",
        ),
    ]
}

const REFUSAL_PROBES: &[&str] = &[
    "refuse.config_string",
    "refuse.harness_type",
    "refuse.spawn_target",
    "refuse.unpermitted_target",
    "refuse.nested_registration",
    "refuse.zero_max_fires",
    "refuse.non_positive_deadline",
    "refuse.absolute_deadline",
    "refuse.unknown_metadata_key",
    "refuse.relative_event_into",
    "refuse.config_condition",
];

struct Prober<'a> {
    context: &'a E2eContext,
    run_id: &'a str,
    root: &'a str,
    deliveries: Arc<Mutex<Vec<Value>>>,
}

impl Prober<'_> {
    /// A control call made the way the subject's own call would reach the
    /// Harness: through the dispatch pipeline, under the root's policy.
    async fn control(&self, function_id: &str, arguments: Value) -> Answer {
        probe::call(
            self.context,
            "harness::function::trigger",
            json!({
                "session_id": self.root,
                "call": { "id": "e2e-binding", "function_id": function_id, "arguments": arguments },
            }),
        )
        .await
    }

    async fn register(&self, arguments: Value) -> (Option<String>, Answer) {
        let answer = self.control("engine::register_trigger", arguments).await;
        let id = answer
            .as_ref()
            .ok()
            .filter(|result| result.get("is_error") != Some(&json!(true)))
            .and_then(|result| result.pointer("/details/subscription_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        (id, answer)
    }

    fn watch(&self, key: &str) -> Value {
        json!({ "scope": scope(self.run_id), "key": key })
    }

    async fn fire(&self, key: &str, value: Value) {
        let _ = probe::call(
            self.context,
            "state::set",
            json!({ "scope": scope(self.run_id), "key": key, "value": value }),
        )
        .await;
    }

    /// The sink deliveries tagged `tag`, once `expected` arrived (or ten
    /// seconds passed) and a further second brought no more.
    async fn delivered(&self, tag: &str, expected: usize) -> Vec<Value> {
        let tagged = || -> Vec<Value> {
            common::lock_unpoisoned(&self.deliveries)
                .iter()
                .filter(|delivery| delivery["tag"] == tag)
                .cloned()
                .collect()
        };
        for _ in 0..40 {
            if tagged().len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        tagged()
    }

    async fn listed(&self) -> Vec<Value> {
        probe::call(
            self.context,
            "harness::triggers::list",
            json!({ "session_id": self.root }),
        )
        .await
        .ok()
        .and_then(|list| list.get("subscriptions").cloned())
        .and_then(|rows| serde_json::from_value(rows).ok())
        .unwrap_or_default()
    }

    async fn is_listed(&self, id: &Option<String>) -> bool {
        self.listed()
            .await
            .iter()
            .any(|row| row.get("subscription_id").and_then(Value::as_str) == id.as_deref())
    }

    async fn refusals(&self, probes: &mut Probes) {
        for (id, arguments, needle) in refusal_cases(self.run_id) {
            let (registered, answer) = self.register(arguments).await;
            probes.check(
                id,
                registered.is_none() && is_refused(&answer, needle),
                &answer,
            );
        }
    }

    async fn call_bindings(&self, probes: &mut Probes) {
        let sink = sink_id(self.run_id);
        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("max"), "function_id": sink,
                "metadata": { "payload": { "tag": "max" } }, "lifecycle": { "max_fires": 2 },
            }))
            .await;
        for value in 1..=3 {
            self.fire("max", json!(value)).await;
            self.delivered("max", value.min(2)).await;
        }
        let max = self.delivered("max", 2).await;
        let listed = self.is_listed(&id).await;
        probes.check(
            "call.max_fires",
            id.is_some() && max.len() == 2 && !listed,
            json!({ "registered": registered, "deliveries": max.len(), "still_listed": listed }),
        );

        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("burst"), "function_id": sink,
                "metadata": { "payload": { "tag": "burst" } },
            }))
            .await;
        for value in 1..=5 {
            self.fire("burst", json!(value)).await;
        }
        let burst = self.delivered("burst", 5).await;
        let values: Vec<Value> = burst
            .iter()
            .map(|delivery| {
                delivery
                    .pointer("/event/new_value")
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect();
        let removed = match &id {
            Some(id) => {
                self.control("engine::unregister_trigger", json!({ "id": id }))
                    .await
            }
            None => Err("never registered".into()),
        };
        probes.check(
            "call.burst",
            id.is_some()
                && burst.len() == 5
                && (1..=5).all(|value| values.contains(&json!(value)))
                && removed
                    .as_ref()
                    .is_ok_and(|result| result.pointer("/details/removed") == Some(&json!(true))),
            json!({ "registered": registered, "values": values, "unregistered": removed }),
        );

        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("expiry"), "function_id": sink,
                "metadata": { "payload": { "tag": "expiry" } },
                "lifecycle": { "expires_in_ms": 1_500 },
            }))
            .await;
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        self.fire("expiry", json!(1)).await;
        let late = self.delivered("expiry", 1).await;
        let listed = self.is_listed(&id).await;
        probes.check(
            "call.expiry",
            id.is_some() && late.is_empty() && !listed,
            json!({ "registered": registered, "deliveries": late.len(), "still_listed": listed }),
        );

        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("scoped"),
                "function_id": scoped_sink_id(self.run_id),
                "metadata": { "payload": { "tag": "scoped", "fs_scope": { "root": "/" } } },
                "lifecycle": { "max_fires": 1 },
            }))
            .await;
        self.fire("scoped", json!(1)).await;
        let scoped = self.delivered("scoped", 1).await;
        let root = scoped
            .first()
            .and_then(|delivery| delivery.pointer("/fs_scope/root"))
            .cloned();
        let expected = workspace(self.run_id).display().to_string();
        probes.check(
            "call.fs_scope",
            id.is_some() && scoped.len() == 1 && root == Some(json!(expected)),
            json!({ "registered": registered, "fs_scope": scoped.first().and_then(|d| d.get("fs_scope")), "expected_root": expected }),
        );
    }

    async fn wakes(&self, probes: &mut Probes) {
        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("wburst"),
                "label": "burst", "once": false,
            }))
            .await;
        for value in 1..=3 {
            self.fire("wburst", json!(value)).await;
        }
        let burst = self.notifications("[notification] burst:", 3).await;
        if let Some(id) = &id {
            let _ = self
                .control("engine::unregister_trigger", json!({ "id": id }))
                .await;
        }
        probes.check(
            "wake.burst",
            id.is_some() && burst.len() == 3,
            json!({ "registered": registered, "notifications": burst }),
        );

        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("wbig"), "label": "big",
            }))
            .await;
        self.fire("wbig", json!("y".repeat(20_000))).await;
        let big = self.notifications("[notification] big:", 1).await;
        let lengths: Vec<usize> = big.iter().map(|text| text.chars().count()).collect();
        probes.check(
            "wake.truncated",
            id.is_some() && big.len() == 1 && big.iter().all(|text| is_truncated_wake(text, "big")),
            json!({ "registered": registered, "lengths": lengths }),
        );

        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("never"), "label": "late",
                "lifecycle": { "expires_in_ms": 1_500 },
            }))
            .await;
        let expired = match &id {
            Some(id) => {
                self.notifications(&format!("(binding {id}) passed its lifecycle deadline"), 1)
                    .await
            }
            None => Vec::new(),
        };
        probes.check(
            "wake.expiry",
            id.is_some()
                && expired.len() == 1
                && expired[0].starts_with("[notification] wake expired unfired:")
                && !self.is_listed(&id).await,
            json!({ "registered": registered, "notifications": expired }),
        );

        let (id, registered) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("wgone"), "label": "gone",
            }))
            .await;
        let removed = match &id {
            Some(id) => {
                probe::call(
                    self.context,
                    "harness::triggers::unregister",
                    json!({ "session_id": self.root, "subscription_id": id }),
                )
                .await
            }
            None => Err("never registered".into()),
        };
        let told = match &id {
            Some(id) => {
                self.notifications(&format!("(binding {id}) was unregistered"), 1)
                    .await
            }
            None => Vec::new(),
        };
        self.fire("wgone", json!(1)).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let fired = self.notifications("[notification] gone:", 0).await;
        probes.check(
            "wake.external_unregister",
            removed.as_ref().is_ok_and(|answer| answer.get("removed") == Some(&json!(true)))
                && told.len() == 1
                && fired.is_empty(),
            json!({ "registered": registered, "removed": removed, "told": told, "fired_after": fired }),
        );
    }

    async fn capacity(&self, probes: &mut Probes) {
        let sink = sink_id(self.run_id);
        let mut active = self.listed().await.len();
        let mut failures = Vec::new();
        let mut slot = 0;
        while active < CAP && slot < CAP * 2 {
            let (id, answer) = self
                .register(json!({
                    "trigger_type": "state", "config": self.watch(&format!("cap{slot}")),
                    "function_id": sink, "metadata": { "payload": { "tag": "cap" } },
                }))
                .await;
            match id {
                Some(_) => active += 1,
                None => failures.push(answer),
            }
            slot += 1;
        }
        let (over, refused) = self
            .register(json!({
                "trigger_type": "state", "config": self.watch("cap-over"),
                "function_id": sink, "metadata": { "payload": { "tag": "cap" } },
            }))
            .await;
        let full = self.listed().await.len();
        probes.check(
            "capacity.cap",
            full == CAP && over.is_none() && is_refused(&refused, "binding cap reached (64 active"),
            json!({ "active": full, "refused": refused, "failed_fills": failures }),
        );

        let teardown = probe::call(
            self.context,
            "harness::teardown",
            json!({ "root_session_id": self.root }),
        )
        .await;
        let left = self.listed().await.len();
        probes.check(
            "capacity.teardown",
            teardown.as_ref().is_ok_and(|answer| {
                answer.get("removed").and_then(Value::as_u64) == Some(full as u64)
            }) && left == 0,
            json!({ "teardown": teardown, "left": left }),
        );
    }

    /// The root's user messages starting with `prefix` (or naming it, for
    /// lifecycle notices), once `expected` of them arrived and the session
    /// settled the turns they started.
    async fn notifications(&self, needle: &str, expected: usize) -> Vec<String> {
        let read = || async {
            self.context
                .transcript(self.root)
                .await
                .map(|transcript| notification_texts(&transcript, needle))
                .unwrap_or_default()
        };
        for _ in 0..60 {
            if read().await.len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ =
            probe::settled_status(self.context, self.root, None, Duration::from_secs(120)).await;
        read().await
    }
}

fn is_refused(answer: &Answer, needle: &str) -> bool {
    answer.as_ref().is_ok_and(|result| {
        result.get("is_error") == Some(&json!(true))
            && result
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(needle))
    })
}

/// User-message texts of `transcript` that start with or contain `needle`.
fn notification_texts(transcript: &Value, needle: &str) -> Vec<String> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.pointer("/content/0/text").and_then(Value::as_str))
        .filter(|text| text.starts_with("[notification]") && text.contains(needle))
        .map(str::to_string)
        .collect()
}

/// A wake whose event was cut: its label prefix, then at most 8,000
/// characters of event and the explicit truncation tail.
fn is_truncated_wake(text: &str, label: &str) -> bool {
    let Some(summary) = text.strip_prefix(&format!("[notification] {label}: ")) else {
        return false;
    };
    let Some(event) = summary.strip_suffix(" …(truncated)") else {
        return false;
    };
    event.chars().count() == WAKE_TEXT_MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::probe::Probe;
    use crate::scenarios::ScenarioId;

    #[test]
    fn a_wake_counts_as_truncated_only_when_cut_to_the_ceiling() {
        let cut = format!(
            "[notification] big: {} …(truncated)",
            "y".repeat(WAKE_TEXT_MAX)
        );
        assert!(is_truncated_wake(&cut, "big"));
        let short = format!("[notification] big: {} …(truncated)", "y".repeat(10));
        assert!(!is_truncated_wake(&short, "big"));
        let whole = format!("[notification] big: {}", "y".repeat(WAKE_TEXT_MAX));
        assert!(!is_truncated_wake(&whole, "big"));
    }

    #[test]
    fn notifications_are_user_messages_from_the_harness() {
        let transcript = json!({ "messages": [
            { "message": { "role": "user", "content": [{ "type": "text", "text": "[notification] burst: {}" }] } },
            { "message": { "role": "user", "content": [{ "type": "text", "text": "burst: typed by a person" }] } },
            { "message": { "role": "assistant", "content": [{ "type": "text", "text": "[notification] burst: x" }] } },
        ] });
        assert_eq!(notification_texts(&transcript, "burst:").len(), 1);
        let refused: Answer = Ok(
            json!({ "is_error": true, "content": [{ "text": "binding cap reached (64 active for this session)" }] }),
        );
        assert!(is_refused(&refused, "binding cap reached (64 active"));
        assert!(!is_refused(
            &Ok(json!({ "is_error": false, "content": [] })),
            "x"
        ));
    }

    #[test]
    fn every_refusal_case_is_scored() {
        let cases: Vec<&str> = refusal_cases("run")
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(cases, REFUSAL_PROBES);
    }

    #[tokio::test]
    async fn no_probes_is_a_prerequisite_failure_and_all_pass_completes() {
        let context = &E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let evaluate = |probes: Vec<Probe>| {
            let observation = ScenarioObservation::for_test(
                ScenarioId::BindingContractProbe,
                "control",
                json!({ "messages": [] }),
                "OK",
                vec![Probes(probes).deliverable(DELIVERABLE_ID, "harness_probes", "e2e_control")],
            );
            async move {
                BindingContractProbe
                    .evaluate(context, &observation, "control")
                    .await
                    .unwrap()
            }
        };
        let none = evaluate(Vec::new()).await;
        assert!(none
            .awards
            .iter()
            .all(|award| award.awarded.unwrap_or(0) == 0));
        assert_eq!(
            none.completion,
            Some(crate::report::CompletionState::Undetermined)
        );
        let ids = REFUSAL_PROBES.iter().copied().chain([
            "call.max_fires",
            "call.burst",
            "call.expiry",
            "call.fs_scope",
            "wake.burst",
            "wake.truncated",
            "wake.expiry",
            "wake.external_unregister",
            "capacity.cap",
            "capacity.teardown",
        ]);
        let all = evaluate(
            ids.map(|id| Probe {
                id: id.into(),
                passed: true,
                observed: Value::Null,
            })
            .collect(),
        )
        .await;
        let points: u16 = all
            .awards
            .iter()
            .filter_map(|award| award.awarded)
            .map(u16::from)
            .sum();
        assert_eq!(points, 100);
    }
}
