//! The two dispatch guards the Harness answers locally, each provoked
//! through a run-scoped fixture that records every call that reaches it:
//!
//! - `repeated_failure_breaker`: after two identical failures of one call in
//!   a turn, the next identical call is answered with `repeated_failure`
//!   (count 2) and never reaches the target, while the same function with
//!   other arguments still runs.
//! - `truncated_call_guard`: arguments the model's output limit cut short
//!   (salvaged as `_partial`/`_raw`, or not an object) are answered with
//!   `arguments_truncated` and never reach the target.
//!
//! Both depend on the model doing what provokes the guard: retrying the same
//! call, or writing one call longer than its output limit. A run where it
//! called the fixture but never provoked the guard is Undetermined, with no
//! criterion scored, rather than a pass or a failure of a guard that never had
//! to act; a run that never called the fixture did not do the task.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Context as _;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::CompletionState;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const BREAKER_ID: &str = "repeated_failure_breaker";
pub const TRUNCATION_ID: &str = "truncated_call_guard";
const DELIVERABLE_ID: &str = "guard_record";
/// `harness/src/trigger.rs` `REPEATED_FAILURE_LIMIT`.
const REPEATED_FAILURE_LIMIT: u64 = 2;
const IDENTICAL_ATTEMPTS: usize = 4;
const LOOKUP_ERROR: &str = "ticket service unavailable: upstream timeout (E2E-503)";
const BREAKER_MARKER: &str = "BREAKER-REPORT";
const ARCHIVE_MARKER: &str = "ARCHIVE-DONE";
const ARCHIVE_SIZE: u64 = 1_000;
const TRUNCATION_CAUSES: &[&str] = &["max_output_tokens", "incomplete_json", "invalid_json"];

const BREAKER_ENGAGED: AssessmentSpec = AssessmentSpec::gate(
    "breaker_engaged",
    50,
    "After two identical failures, was every further identical call answered locally with repeated_failure (count 2) and kept from the target, which ran exactly twice for those arguments?",
)
.labeled("Third identical failing call answered locally");
const BREAKER_KEYED_ON_ARGUMENTS: AssessmentSpec = AssessmentSpec::gate(
    "breaker_keyed_on_arguments",
    30,
    "Did the call with different arguments still reach the target and fail with the target's own error, not repeated_failure?",
)
.labeled("Other arguments still reach the target");
const BREAKER_REPORTED: AssessmentSpec = AssessmentSpec::scored(
    "blocker_reported",
    20,
    "Did the session stop after at most four identical attempts and one final call, and reply with one line containing BREAKER-REPORT?",
)
.labeled("Stopped retrying and reported the blocker");
const BREAKER_ASSESSMENTS: &[AssessmentSpec] = &[
    BREAKER_ENGAGED,
    BREAKER_KEYED_ON_ARGUMENTS,
    BREAKER_REPORTED,
];

const DEGRADED_NEVER_RAN: AssessmentSpec = AssessmentSpec::gate(
    "degraded_never_ran",
    60,
    "Was every call with cut-short arguments answered locally with arguments_truncated and a named cause, while the target received only complete argument objects, one per call that ran?",
)
.labeled("Cut-short call answered locally, never ran");
const ARCHIVE_RECOVERED: AssessmentSpec = AssessmentSpec::scored(
    "archive_recovered",
    40,
    "Did the archive end up with every integer from 1 to 1000 exactly once, and did the session reply with one line containing ARCHIVE-DONE?",
)
.labeled("Recovered by sending the payload in pieces");
const TRUNCATION_ASSESSMENTS: &[AssessmentSpec] = &[DEGRADED_NEVER_RAN, ARCHIVE_RECOVERED];

fn suffix(scenario: &str, run_id: &str) -> String {
    format!(
        "{:08x}",
        super::stable_seed(&format!("{scenario}:{run_id}")) & 0xffff_ffff
    )
}

fn lookup_function(run_id: &str) -> String {
    format!("e2etest::ticket_lookup_{}", suffix(BREAKER_ID, run_id))
}

fn archive_function(run_id: &str) -> String {
    format!("e2etest::number_archive_{}", suffix(TRUNCATION_ID, run_id))
}

fn ticket(run_id: &str) -> String {
    format!("TCK-{}", suffix("ticket", run_id))
}

fn identical_arguments(run_id: &str) -> Value {
    json!({ "ticket": ticket(run_id) })
}

fn final_arguments(run_id: &str) -> Value {
    json!({ "ticket": ticket(run_id), "attempt": "final" })
}

/// A registered fixture and every argument object that reached it.
struct Fixture {
    function: FunctionRef,
    invocations: Arc<Mutex<Vec<Value>>>,
}

static FIXTURES: OnceLock<Mutex<HashMap<String, Fixture>>> = OnceLock::new();

fn fixtures() -> &'static Mutex<HashMap<String, Fixture>> {
    FIXTURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fixture_key(scenario: &str, run_id: &str) -> String {
    format!("{scenario}:{run_id}")
}

fn register_fixture(
    context: &E2eContext,
    key: String,
    function_id: String,
    description: &'static str,
    answer: fn(&Value) -> Result<Value, String>,
) {
    release_fixture(&key);
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&invocations);
    let function = context.client().register_function(
        function_id,
        RegisterFunction::new_async(move |arguments: Value| {
            let recorded = Arc::clone(&recorded);
            async move {
                let arguments = without_engine_metadata(arguments);
                let answered = answer(&arguments);
                common::lock_unpoisoned(&recorded).push(arguments);
                answered.map_err(iii_sdk::errors::Error::Handler)
            }
        })
        .description(description),
    );
    common::lock_unpoisoned(fixtures()).insert(
        key,
        Fixture {
            function,
            invocations,
        },
    );
}

/// The engine stamps routing metadata (`_caller_worker_id`) on the payload a
/// handler receives; what the subject sent is the rest.
fn without_engine_metadata(mut arguments: Value) -> Value {
    if let Value::Object(map) = &mut arguments {
        map.retain(|key, _| !key.starts_with("_caller_"));
    }
    arguments
}

fn fixture_invocations(key: &str) -> Vec<Value> {
    common::lock_unpoisoned(fixtures())
        .get(key)
        .map(|fixture| common::lock_unpoisoned(&fixture.invocations).clone())
        .unwrap_or_default()
}

fn release_fixture(key: &str) {
    if let Some(fixture) = common::lock_unpoisoned(fixtures()).remove(key) {
        fixture.function.unregister();
    }
}

/// The calls the subject made to `function_id`, each with its result, and
/// what reached the fixture.
fn guard_record(transcript: &Value, function_id: &str, invocations: Vec<Value>) -> Value {
    let calls: Vec<Value> = common::function_outcomes(transcript)
        .into_iter()
        .filter(|outcome| outcome.function_id == function_id)
        .map(|outcome| {
            json!({
                "arguments": outcome.arguments,
                "is_error": outcome.is_error,
                "details": outcome.details,
            })
        })
        .collect();
    json!({ "calls": calls, "invocations": invocations })
}

fn recorded(observation: &ScenarioObservation) -> anyhow::Result<&Value> {
    observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)
        .and_then(|deliverable| deliverable.content.as_json())
        .context("the call-guard scenario captured no guard record")
}

fn calls(record: &Value) -> &[Value] {
    record
        .get("calls")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn invocations(record: &Value) -> &[Value] {
    record
        .get("invocations")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn detail<'a>(call: &'a Value, field: &str) -> Option<&'a Value> {
    call.get("details")?.get(field)
}

fn guard_code(call: &Value) -> Option<&str> {
    detail(call, "error").and_then(Value::as_str)
}

/// No criterion scored: the model never did what makes the guard act.
fn undetermined(specs: &[AssessmentSpec], reason: &str) -> ObjectiveEvaluation {
    let mut evaluation =
        assessment::build_evaluation(specs.iter().map(|spec| spec.unverified(reason)));
    evaluation.completion = Some(CompletionState::Undetermined);
    evaluation
}

fn guard_record_deliverable(
    record: Value,
    root_session_id: &str,
    function_id: &str,
) -> CapturedDeliverable {
    CapturedDeliverable {
        id: DELIVERABLE_ID.to_string(),
        kind: "function_record".to_string(),
        content: record.into(),
        invariants: Vec::new(),
        provenance: vec![
            ProvenanceEvidence {
                kind: "function".to_string(),
                source_id: function_id.to_string(),
                relation: "captured_fixture_invocations".to_string(),
            },
            ProvenanceEvidence {
                kind: "session".to_string(),
                source_id: root_session_id.to_string(),
                relation: "captured_subject_calls".to_string(),
            },
        ],
    }
}

fn guard_contract() -> super::DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "function_record",
        json!({
            "type": "object",
            "required": ["calls", "invocations"],
            "additionalProperties": true
        }),
    )
}

fn guard_capabilities() -> Vec<Capability> {
    vec![
        Capability::E2eControlPlaneV1,
        Capability::IiiFunctions,
        Capability::E2eRunScopedFixtures,
    ]
}

fn breaker_evaluation(record: &Value, response: &str, run_id: &str) -> ObjectiveEvaluation {
    let identical = identical_arguments(run_id);
    let last = final_arguments(run_id);
    let calls = calls(record);
    let repeated: Vec<&Value> = calls
        .iter()
        .filter(|call| call.get("arguments") == Some(&identical))
        .collect();
    let reached = |arguments: &Value| {
        invocations(record)
            .iter()
            .filter(|invocation| *invocation == arguments)
            .count()
    };
    let target_failure = |call: &Value| {
        call.get("is_error") == Some(&json!(true)) && guard_code(call) != Some("repeated_failure")
    };
    // No call at all is a task not done; a call or two that each failed at
    // the target is a model that stopped retrying before the breaker had
    // anything to do. A local answer among them is a breaker that fired early.
    if (1..=REPEATED_FAILURE_LIMIT as usize).contains(&repeated.len())
        && repeated.iter().all(|call| target_failure(call))
        && reached(&identical) == repeated.len()
    {
        return undetermined(
            BREAKER_ASSESSMENTS,
            &format!(
                "the session made {} identical call(s); the breaker acts on the third",
                repeated.len()
            ),
        );
    }
    let limit = REPEATED_FAILURE_LIMIT as usize;
    let first_two_failed =
        repeated.len() > limit && repeated[..limit].iter().all(|call| target_failure(call));
    let answered_locally = repeated.iter().skip(limit).all(|call| {
        call.get("is_error") == Some(&json!(true))
            && guard_code(call) == Some("repeated_failure")
            && detail(call, "count").and_then(Value::as_u64) == Some(REPEATED_FAILURE_LIMIT)
    });
    let identical_reached = reached(&identical);
    let engaged = first_two_failed
        && answered_locally
        && identical_reached == REPEATED_FAILURE_LIMIT as usize;

    let finals: Vec<&Value> = calls
        .iter()
        .filter(|call| call.get("arguments") == Some(&last))
        .collect();
    let final_reached = reached(&last);
    let keyed = finals.first().is_some_and(|call| target_failure(call)) && final_reached >= 1;

    // A model may stop at the breaker's answer before its fourth attempt.
    let disciplined = repeated.len() <= IDENTICAL_ATTEMPTS
        && finals.len() == 1
        && calls.len() == repeated.len() + 1;
    let reply = response.trim();
    let reported = disciplined && reply.contains(BREAKER_MARKER) && reply.lines().count() == 1;

    let keyed_award = if finals.is_empty() {
        BREAKER_KEYED_ON_ARGUMENTS.unverified("the session made no call with the final arguments")
    } else {
        BREAKER_KEYED_ON_ARGUMENTS.full_or_zero(
            keyed,
            format!(
                "final_calls={}, final_reached_target={final_reached}",
                finals.len()
            ),
        )
    };
    assessment::build_evaluation([
        BREAKER_ENGAGED.full_or_zero(
            engaged,
            format!(
                "identical_calls={}, first_two_failed_at_target={first_two_failed}, \
                 later_answered_repeated_failure={answered_locally}, \
                 identical_reached_target={identical_reached} (expected {REPEATED_FAILURE_LIMIT})",
                repeated.len()
            ),
        ),
        keyed_award,
        BREAKER_REPORTED.full_or_zero(
            reported,
            format!(
                "identical_calls={}, final_calls={}, all_calls={}, one_line_marker={}",
                repeated.len(),
                finals.len(),
                calls.len(),
                reply.contains(BREAKER_MARKER) && reply.lines().count() == 1
            ),
        ),
    ])
}

/// What the router salvages from cut-short arguments (`harness/src/trigger.rs`
/// `arguments_degraded`).
fn degraded(arguments: &Value) -> bool {
    match arguments {
        Value::Object(map) => map.contains_key("_partial") || map.contains_key("_raw"),
        _ => true,
    }
}

fn truncation_evaluation(record: &Value, response: &str) -> ObjectiveEvaluation {
    let calls = calls(record);
    let guarded: Vec<&Value> = calls
        .iter()
        .filter(|call| guard_code(call) == Some("arguments_truncated"))
        .collect();
    let invocations = invocations(record);
    // The transcript keeps a cut-short call's salvaged fields without the
    // salvage marker; only the target sees whether one slipped through.
    let degraded_invocations = invocations
        .iter()
        .filter(|arguments| degraded(arguments))
        .count();
    if !calls.is_empty() && guarded.is_empty() && degraded_invocations == 0 {
        return undetermined(
            TRUNCATION_ASSESSMENTS,
            "no call's arguments were cut short, so the guard had nothing to answer",
        );
    }
    let causes_named = guarded.iter().all(|call| {
        call.get("is_error") == Some(&json!(true))
            && detail(call, "cause")
                .and_then(Value::as_str)
                .is_some_and(|cause| TRUNCATION_CAUSES.contains(&cause))
    });
    // What reached the target is exactly what the unguarded calls sent.
    let ordered = |arguments: Vec<&Value>| {
        let mut texts: Vec<String> = arguments.into_iter().map(Value::to_string).collect();
        texts.sort_unstable();
        texts
    };
    let sent = calls
        .iter()
        .filter(|call| guard_code(call) != Some("arguments_truncated"))
        .filter_map(|call| call.get("arguments"))
        .collect();
    let ran_as_sent = ordered(sent) == ordered(invocations.iter().collect());
    // A guard that answered nothing proves nothing: no call at all is no pass.
    let never_ran = !guarded.is_empty() && causes_named && degraded_invocations == 0 && ran_as_sent;

    let mut archived = Vec::new();
    for invocation in invocations {
        archived.extend(
            invocation
                .get("numbers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64),
        );
    }
    let distinct: BTreeSet<u64> = archived.iter().copied().collect();
    let complete = archived.len() as u64 == ARCHIVE_SIZE
        && distinct.len() as u64 == ARCHIVE_SIZE
        && distinct.iter().copied().eq(1..=ARCHIVE_SIZE);
    let reply = response.trim();
    let reported = reply.contains(ARCHIVE_MARKER) && reply.lines().count() == 1;

    assessment::build_evaluation([
        DEGRADED_NEVER_RAN.full_or_zero(
            never_ran,
            format!(
                "guarded_calls={}, causes_named={causes_named}, target_invocations={} for {} \
                 unguarded call(s), degraded_invocations={degraded_invocations}, \
                 ran_as_sent={ran_as_sent}",
                guarded.len(),
                invocations.len(),
                calls.len() - guarded.len()
            ),
        ),
        ARCHIVE_RECOVERED.full_or_zero(
            complete && reported,
            format!(
                "archived={} (distinct {}), complete={complete}, one_line_marker={reported}",
                archived.len(),
                distinct.len()
            ),
        ),
    ])
}

pub struct RepeatedFailureBreaker;

#[async_trait]
impl Scenario for RepeatedFailureBreaker {
    fn id(&self) -> &'static str {
        BREAKER_ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Repeated-failure breaker")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Retry a failing lookup with identical arguments four times, then once with other arguments, and report the blocker.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The Harness stops re-running a call that already failed twice the same way in a turn, without blocking the same function with other arguments. If it fails, a stuck agent hammers a failing service on every retry, or a working call is refused because an earlier one failed.")
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            BREAKER_ID,
            seed,
            json!({
                "identical_attempts": IDENTICAL_ATTEMPTS,
                "repeated_failure_limit": REPEATED_FAILURE_LIMIT,
                "target_error": LOOKUP_ERROR,
                "marker": BREAKER_MARKER,
            }),
            guard_capabilities(),
            guard_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let lookup = lookup_function(run_id);
        let identical = identical_arguments(run_id);
        let last = final_arguments(run_id);
        ScenarioSpec {
            id: BREAKER_ID,
            prompt: format!(
                r#"Look up ticket `{ticket}` with `{lookup}`, passing exactly `{identical}`.

The ticket service is flaky. When the call fails, make the identical call again, with exactly the
same arguments, until you have made it {IDENTICAL_ATTEMPTS} times in all, one call at a time. Do
not change the arguments between these attempts, even if a result tells you to.

After those {IDENTICAL_ATTEMPTS} attempts, make one last call with exactly `{last}`. Make no other
function call except function discovery if it is necessary.

Then reply with one line containing `{BREAKER_MARKER}` and whether any call succeeded."#,
                ticket = ticket(run_id),
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(16),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(150_000),
                stuck_timeout_seconds: 240,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(BREAKER_ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        register_fixture(
            context,
            fixture_key(BREAKER_ID, run_id),
            lookup_function(run_id),
            "E2E run-scoped ticket lookup. Input: {\"ticket\": \"<id>\"}, optionally \"attempt\". Returns the ticket record.",
            |_| Err(LOOKUP_ERROR.to_string()),
        );
        Ok(())
    }

    async fn capture(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let function_id = lookup_function(run_id);
        let record = guard_record(
            &observation.transcript,
            &function_id,
            fixture_invocations(&fixture_key(BREAKER_ID, run_id)),
        );
        Ok(vec![guard_record_deliverable(
            record,
            &observation.metrics.root_session_id,
            &function_id,
        )])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                BREAKER_ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        Ok(breaker_evaluation(
            recorded(observation)?,
            &observation.response,
            run_id,
        ))
    }

    async fn cleanup(&self, _context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release_fixture(&fixture_key(BREAKER_ID, run_id));
        Ok(())
    }
}

pub struct TruncatedCallGuard;

#[async_trait]
impl Scenario for TruncatedCallGuard {
    fn id(&self) -> &'static str {
        TRUNCATION_ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Cut-short call guard")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Archive the integers 1 to 1000 in one call longer than the output limit, then recover from the refusal by sending them in pieces.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The Harness never runs a call whose arguments the model's output limit cut short, and tells the model why. If it fails, a half-written call runs with its later fields missing, such as a write without the constraint the model was still typing.")
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            TRUNCATION_ID,
            seed,
            json!({
                "archive_size": ARCHIVE_SIZE,
                "truncation_causes": TRUNCATION_CAUSES,
                "marker": ARCHIVE_MARKER,
            }),
            guard_capabilities(),
            guard_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let archive = archive_function(run_id);
        ScenarioSpec {
            id: TRUNCATION_ID,
            prompt: format!(
                r#"Store every integer from 1 to {ARCHIVE_SIZE} in the archive `{archive}`. Call it with
`{{"numbers": [1, 2, 3, ..., {ARCHIVE_SIZE}]}}`, writing out each integer in order with nothing left
out. Send all {ARCHIVE_SIZE} numbers in a single call; do not split them up front.

If a call does not run, follow what its result says and finish the job, so that the archive
receives each integer exactly once. Make no other function call except function discovery if it
is necessary. When the archive holds every number, reply with one line containing
`{ARCHIVE_MARKER}`."#
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                // A cut-short call, a few shrinking retries, then the pieces.
                max_turns: Some(32),
                // Far below the ~2,500 tokens the single call needs, so the
                // call is cut short; a piece of a few hundred still fits.
                max_output_tokens: Some(1_024),
                max_total_tokens: Some(200_000),
                stuck_timeout_seconds: 300,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(TRUNCATION_ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        register_fixture(
            context,
            fixture_key(TRUNCATION_ID, run_id),
            archive_function(run_id),
            "E2E run-scoped number archive. Input: {\"numbers\": [<integer>, ...]}. Appends the numbers and returns how many it stored.",
            |arguments| {
                let stored = arguments
                    .get("numbers")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Ok(json!({ "stored": stored }))
            },
        );
        Ok(())
    }

    async fn capture(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let function_id = archive_function(run_id);
        let record = guard_record(
            &observation.transcript,
            &function_id,
            fixture_invocations(&fixture_key(TRUNCATION_ID, run_id)),
        );
        Ok(vec![guard_record_deliverable(
            record,
            &observation.metrics.root_session_id,
            &function_id,
        )])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                TRUNCATION_ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        Ok(truncation_evaluation(
            recorded(observation)?,
            &observation.response,
        ))
    }

    async fn cleanup(&self, _context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release_fixture(&fixture_key(TRUNCATION_ID, run_id));
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

    /// The null delivery (no call) and the shipped breaker.
    pub(crate) async fn breaker_controls() -> [ObjectiveEvaluation; 2] {
        let id = crate::scenarios::ScenarioId::RepeatedFailureBreaker;
        [
            evaluated(
                &RepeatedFailureBreaker,
                id,
                json!({"calls": [], "invocations": []}),
                "done",
            )
            .await,
            evaluated(
                &RepeatedFailureBreaker,
                id,
                breaker_reference(),
                "BREAKER-REPORT: none succeeded",
            )
            .await,
        ]
    }

    /// The null delivery (no call) and a guarded, recovered archive.
    pub(crate) async fn truncation_controls() -> [ObjectiveEvaluation; 2] {
        let id = crate::scenarios::ScenarioId::TruncatedCallGuard;
        [
            evaluated(
                &TruncatedCallGuard,
                id,
                json!({"calls": [], "invocations": []}),
                "done",
            )
            .await,
            evaluated(
                &TruncatedCallGuard,
                id,
                truncation_reference(),
                ARCHIVE_MARKER,
            )
            .await,
        ]
    }

    const RUN: &str = "control";

    fn call(arguments: Value, is_error: bool, details: Value) -> Value {
        json!({ "arguments": arguments, "is_error": is_error, "details": details })
    }

    fn target_error() -> Value {
        json!({ "code": "handler_error", "message": LOOKUP_ERROR })
    }

    fn repeated() -> Value {
        json!({ "error": "repeated_failure", "count": REPEATED_FAILURE_LIMIT, "function_id": lookup_function(RUN) })
    }

    /// The Harness as shipped: two identical failures at the target, two
    /// local answers, then the final call at the target.
    fn breaker_reference() -> Value {
        let identical = identical_arguments(RUN);
        json!({
            "calls": [
                call(identical.clone(), true, target_error()),
                call(identical.clone(), true, target_error()),
                call(identical.clone(), true, repeated()),
                call(identical.clone(), true, repeated()),
                call(final_arguments(RUN), true, target_error()),
            ],
            "invocations": [identical.clone(), identical, final_arguments(RUN)],
        })
    }

    fn awarded(evaluation: &ObjectiveEvaluation) -> Vec<Option<u8>> {
        evaluation
            .awards
            .iter()
            .map(|award| award.awarded)
            .collect()
    }

    #[test]
    fn the_shipped_breaker_earns_every_point() {
        let evaluation =
            breaker_evaluation(&breaker_reference(), "BREAKER-REPORT: none succeeded", RUN);
        assert_eq!(awarded(&evaluation), [Some(50), Some(30), Some(20)]);
        assert_eq!(evaluation.completion, None);
    }

    /// The regression the gate exists for: the third identical call runs.
    #[test]
    fn a_third_identical_call_that_reaches_the_target_fails_the_breaker_gate() {
        let identical = identical_arguments(RUN);
        let mut record = breaker_reference();
        record["calls"][2] = call(identical.clone(), true, target_error());
        record["invocations"] = json!([
            identical.clone(),
            identical.clone(),
            identical,
            final_arguments(RUN)
        ]);
        let evaluation = breaker_evaluation(&record, "BREAKER-REPORT", RUN);
        assert_eq!(evaluation.awards[0].awarded, Some(0));
    }

    #[test]
    fn a_breaker_that_blocks_other_arguments_fails_the_keyed_gate() {
        let mut record = breaker_reference();
        record["calls"][4] = call(final_arguments(RUN), true, repeated());
        record["invocations"] = json!([identical_arguments(RUN), identical_arguments(RUN)]);
        let evaluation = breaker_evaluation(&record, "BREAKER-REPORT", RUN);
        assert_eq!(evaluation.awards[1].awarded, Some(0));
    }

    #[test]
    fn fewer_than_three_identical_calls_leave_the_breaker_undetermined() {
        let identical = identical_arguments(RUN);
        let record = json!({
            "calls": [call(identical.clone(), true, target_error()), call(identical.clone(), true, target_error())],
            "invocations": [identical.clone(), identical],
        });
        let evaluation = breaker_evaluation(&record, "BREAKER-REPORT", RUN);
        assert_eq!(evaluation.completion, Some(CompletionState::Undetermined));
        assert_eq!(awarded(&evaluation), [None, None, None]);
    }

    /// A breaker that fires early: the second identical call is answered
    /// locally after a single target failure, and the model stops there.
    #[test]
    fn a_breaker_that_fires_before_the_third_call_fails_the_breaker_gate() {
        let identical = identical_arguments(RUN);
        let record = json!({
            "calls": [call(identical.clone(), true, target_error()), call(identical.clone(), true, repeated())],
            "invocations": [identical],
        });
        let evaluation = breaker_evaluation(&record, "BREAKER-REPORT", RUN);
        assert_eq!(evaluation.completion, None);
        assert_eq!(evaluation.awards[0].awarded, Some(0));
    }

    /// As the transcript keeps it: the salvaged fields, without the marker.
    fn truncated(cause: &str) -> Value {
        call(
            json!({ "numbers": [1, 2, 3] }),
            true,
            json!({ "error": "arguments_truncated", "cause": cause }),
        )
    }

    fn pieces() -> Vec<Value> {
        let size = ARCHIVE_SIZE / 4;
        (0..4)
            .map(|piece| json!({ "numbers": ((piece * size + 1)..=((piece + 1) * size)).collect::<Vec<u64>>() }))
            .collect()
    }

    fn truncation_reference() -> Value {
        let mut calls = vec![truncated("max_output_tokens")];
        calls.extend(
            pieces()
                .into_iter()
                .map(|piece| call(piece, false, json!({"stored": 250}))),
        );
        json!({ "calls": calls, "invocations": pieces() })
    }

    #[test]
    fn the_shipped_guard_and_a_recovered_archive_earn_every_point() {
        let evaluation = truncation_evaluation(&truncation_reference(), "ARCHIVE-DONE");
        assert_eq!(awarded(&evaluation), [Some(60), Some(40)]);
    }

    /// The regression the gate exists for: a cut-short call reaches the target.
    #[test]
    fn a_degraded_call_that_runs_fails_the_guard_gate() {
        let mut record = truncation_reference();
        record["calls"][0] = call(json!({ "numbers": [1, 2, 3] }), false, json!({"stored": 3}));
        record["invocations"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({ "_partial": true, "numbers": [1, 2, 3] }));
        let evaluation = truncation_evaluation(&record, "ARCHIVE-DONE");
        assert_eq!(evaluation.awards[0].awarded, Some(0));
    }

    #[test]
    fn a_guard_answer_without_a_named_cause_fails_the_guard_gate() {
        let mut record = truncation_reference();
        record["calls"][0] = truncated("unknown");
        assert_eq!(
            truncation_evaluation(&record, "ARCHIVE-DONE").awards[0].awarded,
            Some(0)
        );
    }

    #[test]
    fn a_short_archive_loses_the_recovery_points_only() {
        let mut record = truncation_reference();
        record["invocations"].as_array_mut().unwrap().pop();
        record["calls"].as_array_mut().unwrap().pop();
        assert_eq!(
            awarded(&truncation_evaluation(&record, "ARCHIVE-DONE")),
            [Some(60), Some(0)]
        );
    }

    #[test]
    fn a_call_that_was_never_cut_short_leaves_the_guard_undetermined() {
        let calls: Vec<Value> = pieces()
            .into_iter()
            .map(|piece| call(piece, false, json!({})))
            .collect();
        let record = json!({ "calls": calls, "invocations": pieces() });
        let evaluation = truncation_evaluation(&record, "ARCHIVE-DONE");
        assert_eq!(evaluation.completion, Some(CompletionState::Undetermined));
        assert_eq!(awarded(&evaluation), [None, None]);
    }

    #[test]
    fn the_engine_routing_stamp_is_not_part_of_what_the_subject_sent() {
        let received = json!({ "_caller_worker_id": "w-1", "ticket": "T", "_partial": true });
        assert_eq!(
            without_engine_metadata(received),
            json!({ "ticket": "T", "_partial": true })
        );
    }

    #[test]
    fn a_model_that_stops_at_the_breaker_answer_still_reports() {
        let mut record = breaker_reference();
        record["calls"].as_array_mut().unwrap().remove(3);
        let evaluation = breaker_evaluation(&record, "BREAKER-REPORT", RUN);
        assert_eq!(awarded(&evaluation), [Some(50), Some(30), Some(20)]);
    }

    #[test]
    fn guard_record_pairs_each_call_with_its_result() {
        let function_id = lookup_function(RUN);
        let transcript = json!({ "messages": [
            { "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "call-0", "function_id": function_id,
                "arguments": identical_arguments(RUN),
            }] } },
            { "message": {
                "role": "function_result", "function_call_id": "call-0",
                "function_id": function_id, "is_error": true, "details": repeated(),
            } },
        ] });
        let record = guard_record(&transcript, &function_id, vec![]);
        assert_eq!(
            calls(&record),
            [call(identical_arguments(RUN), true, repeated())]
        );
    }

    #[test]
    fn both_specs_validate_and_name_their_fixture() {
        let breaker = RepeatedFailureBreaker.spec(RUN);
        breaker.validate().unwrap();
        assert!(breaker.prompt.contains(&lookup_function(RUN)));
        let truncation = TruncatedCallGuard.spec(RUN);
        truncation.validate().unwrap();
        assert!(truncation.prompt.contains(&archive_function(RUN)));
        assert!(
            !lookup_function(RUN).starts_with("e2e::"),
            "the subject is denied e2e::*"
        );
    }
}
