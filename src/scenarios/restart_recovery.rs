//! `restart_recovery` — a session and a held turn survive worker restarts.
//!
//! Phase 1, between the first two exchanges: the runner restarts
//! session-manager (`compose::restart`), and the session's meta and entries
//! read back byte-identical; the second exchange then continues on them.
//!
//! Phase 2, during the third exchange: the subject calls a run-scoped hold
//! that does not answer, and while it is held the runner restarts the Harness.
//! The new Harness announces `harness::ready` once, the held turn completes
//! within 240 s of the restart with one result per call and one turn
//! completion, and `session::get` reads `done` before anyone calls
//! `harness::status`, which would repair the status itself.
//!
//! The ready, turn-completed and hold registrations are made before either
//! restart, so the run first checks that they survive one: a binding the
//! engine did not hand to the restarted Harness never hears `harness::ready`.
//!
//! The restarts touch the whole stack, so the test runs in a stack group of
//! its own, last in its suite.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::{IIIClient, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::context::E2eContext;
use crate::wire::{TurnCompletedPayload, TurnStatus};

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioExecutionKind, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "restart_recovery";
const DELIVERABLE_ID: &str = "restart_record";
const SESSION_MANAGER: &str = "session-manager";
const HARNESS: &str = "harness";
/// The held turn must complete this long after the Harness restart.
const RECOVERY_LIMIT: Duration = Duration::from_secs(240);
const SESSION_MANAGER_RETURN: Duration = Duration::from_secs(120);
/// How long the projected status may take to land after the turn completes.
const STATUS_PROJECTION: Duration = Duration::from_secs(10);

const STORE_SURVIVES: AssessmentSpec = AssessmentSpec::gate(
    "store_survives_restart",
    20,
    "After the first exchange wrote its row, did a session-manager restart leave the session's meta and entries byte-identical?",
)
.labeled("Session store unchanged by a session-manager restart");
const SESSION_CONTINUES: AssessmentSpec = AssessmentSpec::gate(
    "session_continues",
    15,
    "Did the exchanges before and after the session-manager restart both store their exact rows?",
)
.labeled("The session continues after the restart");
const READY_ONCE: AssessmentSpec = AssessmentSpec::gate(
    "ready_once",
    15,
    "Did a binding registered before the Harness restart hear harness::ready exactly once after it, with the hold function still registered?",
)
.labeled("harness::ready once after the restart");
const HELD_TURN_RECOVERS: AssessmentSpec = AssessmentSpec::gate(
    "held_turn_recovers",
    35,
    "Did the turn held across the Harness restart complete within 240 s of it, with one turn completion, one result per hold call, and the hold's token stored?",
)
.labeled("Held turn completes once after the restart");
const STATUS_DONE: AssessmentSpec = AssessmentSpec::gate(
    "status_done",
    15,
    "Did session::get read the session as done after the recovered turn, before harness::status could repair it?",
)
.labeled("Session status done without a repair");
const ASSESSMENTS: &[AssessmentSpec] = &[
    STORE_SURVIVES,
    SESSION_CONTINUES,
    READY_ONCE,
    HELD_TURN_RECOVERS,
    STATUS_DONE,
];

fn suffix(run_id: &str) -> String {
    format!(
        "{:08x}",
        super::stable_seed(&format!("{ID}:{run_id}")) & 0xffff_ffff
    )
}

fn scope(run_id: &str) -> String {
    format!("e2e-restart-{run_id}")
}

fn row_key(phase: u8) -> String {
    format!("phase-{phase}")
}

fn token(run_id: &str, phase: u8) -> String {
    format!(
        "RST-{:012x}",
        super::stable_seed(&format!("{ID}:{run_id}:{phase}")) & 0xffff_ffff_ffff
    )
}

fn expected_row(run_id: &str, phase: u8) -> Value {
    json!({ "phase": phase, "token": token(run_id, phase) })
}

fn hold_function(run_id: &str) -> String {
    format!("e2etest::restart_hold_{}", suffix(run_id))
}

fn ready_sink(run_id: &str) -> String {
    format!("e2e::restart-ready::{}", suffix(run_id))
}

fn turn_sink(run_id: &str) -> String {
    format!("e2e::restart-turns::{}", suffix(run_id))
}

/// Phase 1 as the runner saw it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct StoreCheck {
    meta_identical: bool,
    entries_identical: bool,
    /// The entries the restart had to keep hold the first exchange's write.
    wrote_first_row: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Completion {
    turn_id: String,
    status: TurnStatus,
    terminal: bool,
    after_restart_ms: u64,
}

/// What the probes heard, captured into the deliverable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct ProbeRecord {
    store: Option<StoreCheck>,
    /// The runner could not take phase 1's measurement: an infrastructure
    /// failure, not the Harness's.
    store_error: Option<String>,
    harness_restarted: bool,
    restart_error: Option<String>,
    ready_after_restart: usize,
    hold_invocations: usize,
    hold_listed_after_restart: Option<bool>,
    completions_after_restart: Vec<Completion>,
    status_after_restart: Option<String>,
}

#[derive(Default)]
struct Probe {
    record: ProbeRecord,
    root_session_id: Option<String>,
    restarted_at: Option<Instant>,
}

struct Runtime {
    functions: Vec<FunctionRef>,
    bindings: Vec<Trigger>,
    probe: Arc<Mutex<Probe>>,
}

static RUNTIMES: OnceLock<Mutex<HashMap<String, Runtime>>> = OnceLock::new();

fn runtimes() -> &'static Mutex<HashMap<String, Runtime>> {
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn probe(run_id: &str) -> Option<Arc<Mutex<Probe>>> {
    common::lock_unpoisoned(runtimes())
        .get(run_id)
        .map(|runtime| Arc::clone(&runtime.probe))
}

fn release_runtime(run_id: &str) {
    if let Some(runtime) = common::lock_unpoisoned(runtimes()).remove(run_id) {
        runtime
            .bindings
            .iter()
            .for_each(|binding| binding.unregister());
        runtime
            .functions
            .iter()
            .for_each(|function| function.unregister());
    }
}

/// `compose::restart` for one container, followed to its outcome when the
/// daemon only accepts the operation.
async fn restart(client: &IIIClient, container: &str) -> anyhow::Result<()> {
    let accepted = common::invoke(
        client,
        "compose::restart",
        json!({ "container": container }),
    )
    .await?;
    let Some(operation_id) = accepted.get("operation_id").and_then(Value::as_str) else {
        return Ok(());
    };
    for _ in 0..240 {
        let operation = common::invoke(
            client,
            "compose::operation",
            json!({ "operation_id": operation_id }),
        )
        .await?;
        match operation.get("status").and_then(Value::as_str) {
            Some("accepted" | "running") => tokio::time::sleep(Duration::from_millis(500)).await,
            Some("failed") => anyhow::bail!(
                "compose::restart {container} failed: {}",
                operation
                    .pointer("/last_event/detail")
                    .and_then(Value::as_str)
                    .unwrap_or("no detail")
            ),
            _ => return Ok(()),
        }
    }
    anyhow::bail!("compose::restart {container} did not finish in 120 s")
}

/// The session's meta and entries, read once they stopped changing: the
/// status projection of the turn that just ended may still be landing.
async fn settled_store(context: &E2eContext, session_id: &str) -> anyhow::Result<(Value, Value)> {
    let read = || async {
        let meta = context
            .trigger_value("session::get", json!({ "session_id": session_id }))
            .await?;
        let entries = context.transcript(session_id).await?;
        anyhow::Ok((meta, entries))
    };
    let mut previous = read().await?;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let current = read().await?;
        if current == previous {
            return Ok(current);
        }
        previous = current;
    }
    anyhow::bail!("session {session_id} kept changing for 10 s before the restart")
}

async fn store_check(
    context: &E2eContext,
    run_id: &str,
    session_id: &str,
) -> anyhow::Result<StoreCheck> {
    let (meta, entries) = settled_store(context, session_id).await?;
    restart(context.client(), SESSION_MANAGER).await?;
    let deadline = Instant::now() + SESSION_MANAGER_RETURN;
    let (meta_after, entries_after) = loop {
        let read = async {
            let meta = context
                .trigger_value("session::get", json!({ "session_id": session_id }))
                .await?;
            let entries = context.transcript(session_id).await?;
            anyhow::Ok((meta, entries))
        };
        match read.await {
            Ok(after) => break after,
            Err(error) if Instant::now() >= deadline => {
                return Err(error.context("session-manager did not come back after its restart"))
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    };
    let first_row = json!({
        "scope": scope(run_id),
        "key": row_key(1),
        "value": expected_row(run_id, 1),
    });
    Ok(StoreCheck {
        meta_identical: serde_json::to_vec(&meta)? == serde_json::to_vec(&meta_after)?,
        entries_identical: serde_json::to_vec(&entries)? == serde_json::to_vec(&entries_after)?,
        wrote_first_row: common::function_calls(&entries)
            .iter()
            .any(|call| call.function_id == "state::set" && call.arguments == first_row),
    })
}

/// How many results each hold call has in the transcript.
fn hold_results(transcript: &Value, hold: &str) -> Vec<usize> {
    let messages: Vec<&Value> = transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .collect();
    common::function_invocations(transcript)
        .into_iter()
        .filter(|invocation| invocation.call.function_id == hold)
        .map(|invocation| {
            messages
                .iter()
                .filter(|message| {
                    message.get("role").and_then(Value::as_str) == Some("function_result")
                        && message.get("function_call_id").and_then(Value::as_str)
                            == invocation.call_id.as_deref()
                })
                .count()
        })
        .collect()
}

fn evaluation(record: &Value, run_id: &str) -> anyhow::Result<ObjectiveEvaluation> {
    let probe: ProbeRecord = serde_json::from_value(record["probe"].clone())?;
    if let Some(error) = probe.store_error.as_ref().or(probe.restart_error.as_ref()) {
        let mut evaluation =
            assessment::prerequisite_failure(ASSESSMENTS, "restart", error.clone());
        evaluation.infrastructure_error = Some(format!(
            "restart_recovery could not restart a worker: {error}"
        ));
        return Ok(evaluation);
    }
    let row = |phase: u8| record["rows"][row_key(phase)] == expected_row(run_id, phase);
    let store = probe.store.clone().unwrap_or_default();
    let store_survives = store.wrote_first_row && store.meta_identical && store.entries_identical;

    let ready_once = probe.harness_restarted
        && probe.ready_after_restart == 1
        && probe.hold_listed_after_restart == Some(true);

    let hold_results: Vec<usize> = serde_json::from_value(record["hold_results"].clone())?;
    let terminal: Vec<&Completion> = probe
        .completions_after_restart
        .iter()
        .filter(|completion| completion.terminal)
        .collect();
    let completed_in_time = matches!(terminal.as_slice(), [completion]
        if completion.status == TurnStatus::Completed
            && completion.after_restart_ms <= RECOVERY_LIMIT.as_millis() as u64);
    let one_result_each =
        !hold_results.is_empty() && hold_results.iter().all(|results| *results == 1);
    let recovered = probe.harness_restarted && completed_in_time && one_result_each && row(3);

    let status_done =
        probe.harness_restarted && probe.status_after_restart.as_deref() == Some("done");

    Ok(assessment::build_evaluation([
        STORE_SURVIVES.full_or_zero(
            store_survives,
            format!(
                "first_row_written={}, meta_identical={}, entries_identical={}",
                store.wrote_first_row, store.meta_identical, store.entries_identical
            ),
        ),
        SESSION_CONTINUES.full_or_zero(
            row(1) && row(2),
            format!("phase_1_row={}, phase_2_row={}", row(1), row(2)),
        ),
        READY_ONCE.full_or_zero(
            ready_once,
            format!(
                "harness_restarted={}, ready_after_restart={}, hold_listed_after_restart={:?}",
                probe.harness_restarted, probe.ready_after_restart, probe.hold_listed_after_restart
            ),
        ),
        HELD_TURN_RECOVERS.full_or_zero(
            recovered,
            format!(
                "terminal_completions_after_restart={:?}, results_per_hold_call={hold_results:?}, \
                 hold_invocations={}, phase_3_row={}",
                terminal
                    .iter()
                    .map(|completion| (completion.status, completion.after_restart_ms))
                    .collect::<Vec<_>>(),
                probe.hold_invocations,
                row(3)
            ),
        ),
        STATUS_DONE.full_or_zero(
            status_done,
            format!("session_get_status={:?}", probe.status_after_restart),
        ),
    ]))
}

fn register_probes(context: &E2eContext, run_id: &str) -> anyhow::Result<Runtime> {
    let probe = Arc::new(Mutex::new(Probe::default()));
    let release = Arc::new(Notify::new());
    let client = context.client().clone();
    let hold = hold_function(run_id);
    let mut functions = Vec::new();

    // The hold answers its first call only once the restarted Harness is
    // ready (or the recovery limit passed); a redriven or repeated call
    // answers at once.
    let (held, released, restarter, answer) = (
        Arc::clone(&probe),
        Arc::clone(&release),
        client.clone(),
        token(run_id, 3),
    );
    functions.push(
        context.client().register_function(
            hold.clone(),
            RegisterFunction::new_async(move |_arguments: Value| {
                let (held, released, restarter, answer) = (
                    Arc::clone(&held),
                    Arc::clone(&released),
                    restarter.clone(),
                    answer.clone(),
                );
                async move {
                    let first = {
                        let mut probe = common::lock_unpoisoned(&held);
                        probe.record.hold_invocations += 1;
                        let first = probe.restarted_at.is_none();
                        if first {
                            probe.restarted_at = Some(Instant::now());
                            probe.record.harness_restarted = true;
                        }
                        first
                    };
                    if first {
                        let (failed, unblocked) = (Arc::clone(&held), Arc::clone(&released));
                        tokio::spawn(async move {
                            if let Err(error) = restart(&restarter, HARNESS).await {
                                common::lock_unpoisoned(&failed).record.restart_error =
                                    Some(format!("{error:#}"));
                                unblocked.notify_one();
                            }
                        });
                        let _ = tokio::time::timeout(RECOVERY_LIMIT, released.notified()).await;
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "token": answer }))
                }
            })
            .description(
                "E2E run-scoped hold. Input: {}. Answers with a token after a while; wait for it.",
            ),
        ),
    );

    let (heard, released, lister, hold_id) = (
        Arc::clone(&probe),
        Arc::clone(&release),
        client.clone(),
        hold.clone(),
    );
    let ready_id = ready_sink(run_id);
    functions.push(
        context.client().register_function(
            ready_id.clone(),
            RegisterFunction::new_async(move |_payload: Value| {
                let (heard, released, lister, hold_id) = (
                    Arc::clone(&heard),
                    Arc::clone(&released),
                    lister.clone(),
                    hold_id.clone(),
                );
                async move {
                    let first_after_restart = {
                        let mut probe = common::lock_unpoisoned(&heard);
                        if probe.restarted_at.is_some() {
                            probe.record.ready_after_restart += 1;
                        }
                        probe.record.ready_after_restart == 1 && probe.restarted_at.is_some()
                    };
                    if first_after_restart {
                        tokio::spawn(async move {
                            let listed = common::invoke(
                                &lister,
                                "engine::functions::list",
                                json!({ "include_internal": true }),
                            )
                            .await
                            .ok()
                            .map(|listed| listed.to_string().contains(&format!("\"{hold_id}\"")));
                            common::lock_unpoisoned(&heard)
                                .record
                                .hold_listed_after_restart = listed;
                            released.notify_one();
                        });
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "ok": true }))
                }
            })
            .description("Counts harness::ready for the E2E restart test.")
            .metadata(json!({ "internal": true })),
        ),
    );

    let (heard, reader) = (Arc::clone(&probe), client.clone());
    let turns_id = turn_sink(run_id);
    functions.push(
        context.client().register_function(
            turns_id.clone(),
            RegisterFunction::new_async(move |payload: Value| {
                let (heard, reader) = (Arc::clone(&heard), reader.clone());
                async move {
                    let Ok(event) = serde_json::from_value::<TurnCompletedPayload>(payload) else {
                        return Ok::<Value, iii_sdk::errors::Error>(json!({ "ok": false }));
                    };
                    let read_status = {
                        let mut probe = common::lock_unpoisoned(&heard);
                        let root =
                            probe.root_session_id.as_deref() == Some(event.session_id.as_str());
                        match probe.restarted_at.filter(|_| root) {
                            Some(restarted_at) => {
                                probe.record.completions_after_restart.push(Completion {
                                    turn_id: event.turn_id.clone(),
                                    status: event.status,
                                    terminal: event.terminal,
                                    after_restart_ms: restarted_at.elapsed().as_millis() as u64,
                                });
                                event.terminal
                            }
                            None => false,
                        }
                    };
                    if read_status {
                        // Before the runner's harness::status, which re-projects it.
                        tokio::spawn(async move {
                            let deadline = Instant::now() + STATUS_PROJECTION;
                            loop {
                                let status = common::invoke(
                                    &reader,
                                    "session::get",
                                    json!({ "session_id": event.session_id }),
                                )
                                .await
                                .ok()
                                .and_then(|got| {
                                    got.pointer("/meta/status")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                });
                                let settled =
                                    status.as_deref().is_some_and(|status| status != "working");
                                if settled || Instant::now() >= deadline {
                                    common::lock_unpoisoned(&heard).record.status_after_restart =
                                        status;
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        });
                    }
                    Ok(json!({ "ok": true }))
                }
            })
            .description("Records the root's turn completions for the E2E restart test.")
            .metadata(json!({ "internal": true })),
        ),
    );

    let bindings = vec![
        context
            .client()
            .register_trigger(RegisterTriggerInput::new(
                "harness::ready",
                ready_id,
                json!({}),
            ))?,
        context
            .client()
            .register_trigger(RegisterTriggerInput::new(
                "harness::turn-completed",
                turns_id,
                json!({}),
            ))?,
    ];
    Ok(Runtime {
        functions,
        bindings,
        probe,
    })
}

pub struct RestartRecovery;

#[async_trait]
impl Scenario for RestartRecovery {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Recovery across worker restarts")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Store a row, survive a session-manager restart and store another, then wait on a held call while the Harness restarts and store the token it returns.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("Sessions and in-flight turns survive restarts of session-manager and the Harness: the transcript is kept as it was, the Harness comes back ready once, and a turn held across the restart finishes once with its session marked done. If it fails, a deploy or a crash loses conversations, leaves turns hanging, or leaves sessions stuck as working.")
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::ScriptedDialogue
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "restarted": [SESSION_MANAGER, HARNESS],
                "recovery_limit_seconds": RECOVERY_LIMIT.as_secs(),
                "rows": [row_key(1), row_key(2), row_key(3)],
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::HarnessScriptedDialogueV1,
                Capability::IiiCompose,
                Capability::IiiFunctions,
                Capability::IiiState,
                Capability::IiiTriggers,
                Capability::E2eRunScopedFixtures,
            ],
            super::validation_loop::validation_contract(
                DELIVERABLE_ID,
                "restart_record",
                json!({
                    "type": "object",
                    "required": ["probe", "rows", "hold_results"],
                    "additionalProperties": true
                }),
            ),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let scope = scope(run_id);
        let first = expected_row(run_id, 1);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Write exactly `{first}` with `state::set` to scope `{scope}`, key `{key}`. Then \
                 reply with one line containing `RESTART-1`. Make no other function call except \
                 function discovery if it is necessary.",
                key = row_key(1),
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(16),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(200_000),
                // The held exchange waits out a Harness restart.
                stuck_timeout_seconds: 420,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        let scope = scope(run_id);
        vec![
            format!(
                "Write exactly `{second}` to scope `{scope}`, key `{key}`. Then reply with one \
                 line containing `RESTART-2`.",
                second = expected_row(run_id, 2),
                key = row_key(2),
            ),
            format!(
                "Call `{hold}` once with `{{}}`. It answers with a token after a while; wait for \
                 it, and if the call fails, call it once more. Then write exactly \
                 `{{\"phase\": 3, \"token\": \"<the token it returned>\"}}` to scope `{scope}`, \
                 key `{key}`, and reply with one line containing `RESTART-3`.",
                hold = hold_function(run_id),
                key = row_key(3),
            ),
        ]
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release_runtime(run_id);
        let runtime = register_probes(context, run_id)?;
        common::lock_unpoisoned(runtimes()).insert(run_id.to_string(), runtime);
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        exchange: usize,
    ) {
        let Some(probe) = probe(run_id) else {
            return;
        };
        common::lock_unpoisoned(&probe).root_session_id = Some(session_id.to_string());
        if exchange != 0 {
            return;
        }
        let checked = store_check(context, run_id, session_id).await;
        let mut probe = common::lock_unpoisoned(&probe);
        match checked {
            Ok(check) => probe.record.store = Some(check),
            Err(error) => probe.record.store_error = Some(format!("{error:#}")),
        }
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let probe = probe(run_id)
            .map(|probe| common::lock_unpoisoned(&probe).record.clone())
            .context("restart_recovery has no probes for this run")?;
        let scope = scope(run_id);
        let mut rows = serde_json::Map::new();
        for phase in 1..=3 {
            rows.insert(
                row_key(phase),
                common::get_state(context, &scope, &row_key(phase)).await?,
            );
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "restart_record".to_string(),
            content: json!({
                "probe": probe,
                "rows": rows,
                "hold_results": hold_results(&observation.transcript, &hold_function(run_id)),
            })
            .into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: scope,
                    relation: "captured_phase_rows".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_restart_probes".to_string(),
                },
            ],
        }])
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
        let record = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .and_then(|deliverable| deliverable.content.as_json())
            .context("restart_recovery captured no restart record")?;
        evaluation(record, run_id)
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release_runtime(run_id);
        let scope = scope(run_id);
        for phase in 1..=3 {
            let _: Value = context
                .trigger_value(
                    "state::delete",
                    json!({ "scope": scope, "key": row_key(phase) }),
                )
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `deliverable` as `capture` records it, scored by `evaluate`.
    async fn evaluated(
        scenario: &dyn Scenario,
        id: crate::scenarios::ScenarioId,
        deliverable: Value,
        response: &str,
    ) -> ObjectiveEvaluation {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let observation = ScenarioObservation::for_test(
            id,
            "control",
            json!({ "messages": [] }),
            response,
            vec![CapturedDeliverable {
                id: DELIVERABLE_ID.to_string(),
                kind: "record".to_string(),
                content: deliverable.into(),
                invariants: Vec::new(),
                provenance: Vec::new(),
            }],
        );
        scenario
            .evaluate(&context, &observation, "control")
            .await
            .unwrap()
    }

    /// The null delivery (the runner still restarts session-manager) and a
    /// sound stack.
    pub(crate) async fn controls() -> [ObjectiveEvaluation; 2] {
        let id = crate::scenarios::ScenarioId::RestartRecovery;
        [
            evaluated(&RestartRecovery, id, null(), "done").await,
            evaluated(&RestartRecovery, id, reference(), "RESTART-3").await,
        ]
    }

    fn null() -> Value {
        let probe = ProbeRecord {
            store: Some(StoreCheck {
                meta_identical: true,
                entries_identical: true,
                wrote_first_row: false,
            }),
            ..ProbeRecord::default()
        };
        json!({ "probe": probe, "rows": {}, "hold_results": [] })
    }

    const RUN: &str = "control";

    fn completion(status: TurnStatus, after_restart_ms: u64) -> Completion {
        Completion {
            turn_id: "turn-3".into(),
            status,
            terminal: true,
            after_restart_ms,
        }
    }

    /// The run a sound stack leaves behind.
    fn reference() -> Value {
        let probe = ProbeRecord {
            store: Some(StoreCheck {
                meta_identical: true,
                entries_identical: true,
                wrote_first_row: true,
            }),
            store_error: None,
            harness_restarted: true,
            restart_error: None,
            ready_after_restart: 1,
            hold_invocations: 2,
            hold_listed_after_restart: Some(true),
            completions_after_restart: vec![completion(TurnStatus::Completed, 95_000)],
            status_after_restart: Some("done".into()),
        };
        json!({
            "probe": probe,
            "rows": {
                "phase-1": expected_row(RUN, 1),
                "phase-2": expected_row(RUN, 2),
                "phase-3": expected_row(RUN, 3),
            },
            "hold_results": [1],
        })
    }

    fn awarded(record: &Value) -> Vec<Option<u8>> {
        evaluation(record, RUN)
            .unwrap()
            .awards
            .iter()
            .map(|award| award.awarded)
            .collect()
    }

    #[test]
    fn a_sound_stack_earns_every_point() {
        assert_eq!(
            awarded(&reference()),
            [Some(20), Some(15), Some(15), Some(35), Some(15)]
        );
    }

    /// The runner restarts session-manager whatever the subject does, so the
    /// store gate also needs the subject's first write.
    #[test]
    fn a_null_agent_earns_nothing() {
        assert_eq!(awarded(&null()), [Some(0); 5]);
    }

    #[test]
    fn a_changed_entry_fails_the_store_gate() {
        let mut record = reference();
        record["probe"]["store"]["entries_identical"] = json!(false);
        assert_eq!(awarded(&record)[0], Some(0));
    }

    #[test]
    fn a_second_ready_or_a_lost_binding_fails_the_ready_gate() {
        for ready in [0, 2] {
            let mut record = reference();
            record["probe"]["ready_after_restart"] = json!(ready);
            assert_eq!(awarded(&record)[2], Some(0), "ready={ready}");
        }
    }

    #[test]
    fn a_late_a_doubled_or_a_failed_completion_fails_the_recovery_gate() {
        let late = vec![completion(TurnStatus::Completed, 241_000)];
        let doubled = vec![
            completion(TurnStatus::Completed, 90_000),
            completion(TurnStatus::Completed, 91_000),
        ];
        let failed = vec![completion(TurnStatus::Failed, 90_000)];
        for completions in [late, doubled, failed] {
            let mut record = reference();
            record["probe"]["completions_after_restart"] = json!(completions);
            assert_eq!(awarded(&record)[3], Some(0), "{completions:?}");
        }
        let mut two_results = reference();
        two_results["hold_results"] = json!([2]);
        assert_eq!(awarded(&two_results)[3], Some(0));
    }

    #[test]
    fn a_session_left_working_fails_the_status_gate() {
        let mut record = reference();
        record["probe"]["status_after_restart"] = json!("working");
        assert_eq!(awarded(&record)[4], Some(0));
    }

    #[test]
    fn a_restart_the_runner_could_not_make_is_an_infrastructure_error() {
        let mut record = reference();
        record["probe"]["store_error"] = json!("compose::restart session-manager failed");
        let evaluation = evaluation(&record, RUN).unwrap();
        assert!(evaluation.infrastructure_error.is_some());
    }

    #[test]
    fn hold_results_count_the_results_of_each_hold_call() {
        let hold = hold_function(RUN);
        let transcript = json!({ "messages": [
            { "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "call-0", "function_id": hold, "arguments": {},
            }] } },
            { "message": { "role": "function_result", "function_call_id": "call-0", "function_id": hold, "is_error": false } },
            { "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "call-1", "function_id": "state::set", "arguments": {},
            }] } },
        ] });
        assert_eq!(hold_results(&transcript, &hold), [1]);
    }

    #[test]
    fn the_dialogue_names_its_hold_and_rows() {
        let spec = RestartRecovery.spec(RUN);
        spec.validate().unwrap();
        let followups = RestartRecovery.dialogue_followups(RUN);
        assert_eq!(followups.len(), 2);
        assert!(followups[1].contains(&hold_function(RUN)));
        assert!(
            !hold_function(RUN).starts_with("e2e::"),
            "the subject is denied e2e::*"
        );
    }
}
