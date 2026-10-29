//! Invariants a correct stack keeps on every run, whatever the model did, read
//! from the evidence the suite already captured: the root session's transcript
//! and the session-tree metrics. The suite decides what each one costs the run.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::report::{E2eRunReport, RunStatus};
use crate::suite::SubjectConfig;

/// The window the context-manager falls back to when it cannot resolve a
/// model's limits (`allow_fallback_limits`, 8192/1024).
const FALLBACK_CONTEXT_WINDOW: u64 = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessInvariant {
    /// A message whose calls the Harness dispatched (some have results) left
    /// one without a result: the persisted history is malformed.
    LostFunctionResult,
    /// A session's context was fit into the context-manager's fallback limits
    /// while the model's catalog window is far larger: the router could not
    /// resolve the model's limits (unreachable, or the stack names a model it
    /// does not list), so compaction fires on a fraction of the window.
    FallbackContextLimits,
    /// The run's tokens or a priced run's cost were not counted, or a
    /// generation that ended or called a function carries no usage.
    UsageUnmetered,
    /// The counters disagree: the sessions' costs do not add up to the total,
    /// a session's running cost drifts from its metered cost, or `free` is
    /// not what the window leaves.
    UsageMisaccounted,
    /// A generation ran on a model or provider its session was not routed to.
    ModelMisrouted,
    /// The subject disabled thinking and still reasoned.
    ReasoningWhileDisabled,
    /// A transient resume in a turn that neither failed nor stopped has no
    /// `recovered` row of its attempt or a later one: the Harness lost the
    /// recovery's outcome.
    UnrecoveredTransientResume,
    /// A turn resumed more often than its `max_transient_resumes`.
    TransientResumesOverLimit,
    /// A `trigger_fired` record has an outcome outside `TriggerOutcome`, or
    /// retired its binding without a `retirement_reason`.
    MalformedTriggerRecord,
    /// A wake binding's `delivered` records and the `e_fire_` entries it
    /// injected differ in number: a wake was lost or doubled.
    WakeDeliveryMismatch,
    /// A `once` binding was delivered more than once.
    OnceBindingRedelivered,
    /// A binding of the run outlived `harness::teardown`, on the engine or in
    /// the binding store.
    LeakedTriggerBinding,
    /// A session froze a built-in prompt other than the default identity (or
    /// the subject's agent profile prompt).
    FrozenPromptDrift,
    /// The runtime context frozen into a session's prompt misnames the
    /// session, the working directory, the co-author or the dispatch policy.
    RuntimeContextMalformed,
    /// An English prompt pinned another response language, or none.
    ResponseLanguageMispinned,
    /// A turn record holds its prompt inline, or names a body the prompt
    /// store does not hold under that hash.
    PromptRefBroken,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessViolation {
    pub invariant: HarnessInvariant,
    pub detail: String,
}

/// Checked only on a run that passed: a stopped or failed run can leave what
/// an invariant forbids without a defect.
pub fn violations(report: &E2eRunReport, subject: &SubjectConfig) -> Vec<HarnessViolation> {
    if report.status != RunStatus::Passed {
        return Vec::new();
    }
    let messages = report
        .transcript
        .as_ref()
        .and_then(|transcript| transcript.get("messages"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut violations = lost_function_results(messages);
    violations.extend(fallback_context_limits(report, subject));
    violations.extend(crate::usage_invariants::violations(report, subject));
    violations.extend(crate::transient_recovery::violations(
        report.transient_recovery.as_deref().unwrap_or_default(),
    ));
    violations.extend(
        report
            .trigger_bindings
            .iter()
            .flat_map(|bindings| bindings.violations.iter().cloned()),
    );
    violations.extend(crate::prompt_surface::violations(report, subject));
    violations
}

/// The turn loop answers every call of a message before the next step, so a
/// message with some calls answered and others not lost a result. A message
/// with none answered was never dispatched: the Harness persists every step
/// with `stop_reason` "end", and a stream cut mid-call keeps its partial calls
/// while a transient resume asks the model again.
fn lost_function_results(messages: &[Value]) -> Vec<HarnessViolation> {
    let results = &messages
        .iter()
        .filter(|entry| text(entry, "/message/role") == Some("function_result"))
        .filter_map(|entry| text(entry, "/message/function_call_id"))
        .collect::<BTreeSet<_>>();
    messages
        .iter()
        .filter(|entry| text(entry, "/message/role") == Some("assistant"))
        .flat_map(|entry| {
            let calls = entry
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("function_call"))
                .filter_map(|block| block.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>();
            let dispatched = calls.iter().any(|id| results.contains(id));
            calls
                .into_iter()
                .filter(move |id| dispatched && !results.contains(id))
                .map(move |id| HarnessViolation {
                    invariant: HarnessInvariant::LostFunctionResult,
                    detail: format!(
                        "call {id} in {} has no function_result",
                        text(entry, "/entry_id").unwrap_or("an unnamed entry")
                    ),
                })
        })
        .collect()
}

/// Only sessions on the subject's own model: a child spawned on another model
/// has a window this run never resolved.
fn fallback_context_limits(
    report: &E2eRunReport,
    subject: &SubjectConfig,
) -> Vec<HarnessViolation> {
    // Four fallback windows of margin: a small model's reserve and thinking
    // budget can legitimately leave it near 8192, a 32k one cannot.
    let Some(window) = subject
        .context_window
        .filter(|window| *window >= 4 * FALLBACK_CONTEXT_WINDOW)
    else {
        return Vec::new();
    };
    let Some(metrics) = report.metrics.as_ref() else {
        return Vec::new();
    };
    metrics
        .by_session
        .iter()
        .filter_map(|session| {
            let context = session.context.as_ref()?;
            if text(context, "/model") != Some(subject.model.as_str()) {
                return None;
            }
            let usable = context.get("usable")?.as_u64()?;
            let output = context.get("effective_max_output_tokens")?.as_u64()?;
            (usable.saturating_add(output) <= FALLBACK_CONTEXT_WINDOW).then(|| HarnessViolation {
                invariant: HarnessInvariant::FallbackContextLimits,
                detail: format!(
                    "session {} ran with {usable} usable + {output} output tokens on a \
                     {window}-token model",
                    session.session_id
                ),
            })
        })
        .collect()
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::wire::{Observed, SessionMetricsPayload, SessionUsage, SessionUsageTotals};

    fn subject(context_window: Option<u64>) -> SubjectConfig {
        SubjectConfig {
            model: "deepseek/deepseek-flash".into(),
            provider: "deepseek".into(),
            agent: None,
            thinking_level: None,
            provider_options: None,
            priced: None,
            context_window,
        }
    }

    fn report(status: RunStatus, messages: Value) -> E2eRunReport {
        let mut report = E2eRunReport::new("run".into(), "a".into(), 1, "root".into(), "p".into());
        report.status = status;
        report.transcript = Some(json!({ "messages": messages }));
        report
    }

    /// The Harness persists every step with `stop_reason` "end".
    fn assistant(entry_id: &str, call_ids: &[&str]) -> Value {
        let calls = call_ids
            .iter()
            .map(|id| json!({ "type": "function_call", "id": id, "function_id": "state::get" }))
            .collect::<Vec<_>>();
        json!({ "entry_id": entry_id, "message": {
            "role": "assistant", "stop_reason": "end", "content": calls,
        } })
    }

    fn result(call_id: &str) -> Value {
        json!({ "message": { "role": "function_result", "function_call_id": call_id } })
    }

    fn invariants(violations: &[HarnessViolation]) -> Vec<HarnessInvariant> {
        violations.iter().map(|v| v.invariant).collect()
    }

    #[test]
    fn a_dispatched_call_without_a_result_is_a_lost_result() {
        let report = report(
            RunStatus::Passed,
            json!([assistant("e_t1_1_assistant", &["c1", "c2"]), result("c1")]),
        );

        let violations = violations(&report, &subject(None));

        assert_eq!(
            invariants(&violations),
            [HarnessInvariant::LostFunctionResult]
        );
        assert!(violations[0].detail.contains("call c2 in e_t1_1_assistant"));
    }

    #[test]
    fn a_stream_cut_mid_call_was_never_dispatched() {
        // The partial keeps its call; the Harness records the recovery and asks
        // the model again instead of dispatching it.
        let report = report(
            RunStatus::Passed,
            json!([
                assistant("e_t1_1_assistant", &["c1"]),
                { "entry_id": "e_t1_transient_recovery_1_recovering",
                  "custom": { "custom_type": "recovery" } },
                { "entry_id": "e_t1_transient_resume_1", "message": { "role": "user" } },
                assistant("e_t1_2_assistant", &["c2"]),
                result("c2"),
            ]),
        );

        assert!(violations(&report, &subject(None)).is_empty());
    }

    #[test]
    fn invariants_are_checked_only_on_a_passed_run() {
        let report = report(
            RunStatus::ResourceLimit,
            json!([assistant("e_t1_1_assistant", &["c1", "c2"]), result("c1")]),
        );

        assert!(violations(&report, &subject(None)).is_empty());
    }

    fn with_contexts(mut report: E2eRunReport, contexts: Vec<Value>) -> E2eRunReport {
        let by_session = contexts
            .into_iter()
            .enumerate()
            .map(|(index, context)| SessionUsage {
                session_id: format!("s{index}"),
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
                by_model: Vec::new(),
                context: Some(context),
            })
            .collect();
        report.metrics = Some(Observed::from_normalized(SessionMetricsPayload {
            root_session_id: "root".into(),
            complete: true,
            totals: SessionUsageTotals {
                input_tokens: Some(1),
                output_tokens: Some(1),
                ..SessionUsageTotals::default()
            },
            by_session,
            traces: None,
        }));
        report
    }

    fn context(model: &str, usable: u64, output: u64) -> Value {
        json!({ "model": model, "usable": usable, "effective_max_output_tokens": output })
    }

    #[test]
    fn the_subject_model_fit_into_the_fallback_limits_is_flagged() {
        let report = with_contexts(
            report(RunStatus::Passed, json!([])),
            vec![
                context("deepseek/deepseek-flash", 6_349, 1_024),
                context("deepseek/deepseek-flash", 100_000, 8_000),
            ],
        );

        let violations = violations(&report, &subject(Some(128_000)));

        assert_eq!(
            invariants(&violations),
            [HarnessInvariant::FallbackContextLimits]
        );
        assert!(violations[0].detail.contains("session s0"));
    }

    #[test]
    fn fallback_limits_need_a_large_subject_window_and_the_subject_model() {
        let fallback = || {
            with_contexts(
                report(RunStatus::Passed, json!([])),
                vec![context("deepseek/deepseek-flash", 6_349, 1_024)],
            )
        };
        let other_model = with_contexts(
            report(RunStatus::Passed, json!([])),
            vec![context("other/tiny", 6_349, 1_024)],
        );

        for violations in [
            violations(&fallback(), &subject(Some(16_000))),
            violations(&fallback(), &subject(None)),
            violations(&other_model, &subject(Some(128_000))),
        ] {
            assert!(violations.is_empty());
        }
    }
}
