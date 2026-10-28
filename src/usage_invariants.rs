//! Usage, cost and routing invariants, read from the root transcript, the
//! descendant transcripts `metrics.by_session` names, and the metrics
//! themselves. `harness_invariants::violations` checks them on a passed run.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::Value;

use crate::context::E2eContext;
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::E2eRunReport;
use crate::suite::SubjectConfig;
use crate::wire::SessionMetricsPayload;

/// Fetches every descendant session's transcript the metrics name, before
/// teardown removes them.
pub async fn child_transcripts(
    context: &E2eContext,
    metrics: &SessionMetricsPayload,
) -> Result<BTreeMap<String, Value>> {
    let mut transcripts = BTreeMap::new();
    for session in &metrics.by_session {
        if session.session_id != metrics.root_session_id {
            let transcript = context.transcript(&session.session_id).await?;
            transcripts.insert(session.session_id.clone(), transcript);
        }
    }
    Ok(transcripts)
}

pub fn violations(report: &E2eRunReport, subject: &SubjectConfig) -> Vec<HarnessViolation> {
    let Some(metrics) = report.metrics.as_deref() else {
        return Vec::new();
    };
    let root = metrics.root_session_id.as_str();
    let transcripts = report
        .transcript
        .iter()
        .map(|transcript| (root, transcript))
        .chain(
            report
                .child_transcripts
                .iter()
                .map(|(session, transcript)| (session.as_str(), transcript)),
        )
        .collect::<Vec<_>>();
    let unreported = transcripts
        .iter()
        .flat_map(|(_, transcript)| generations(transcript))
        .filter(|(_, message)| !message.get("usage").is_some_and(Value::is_object))
        .count();
    let mut violations = metering(metrics, subject, unreported);
    violations.extend(
        transcripts
            .iter()
            .filter_map(|(session, transcript)| unreported_usage(session, transcript)),
    );
    // A live partial snapshot (`complete: false`) read counters mid-update.
    if metrics.complete {
        violations.extend(accounting(metrics));
    }
    violations.extend(routing(metrics, subject, &transcripts));
    violations.extend(reasoning(report, metrics, subject));
    violations
}

/// The Harness nulls a total that more than one generation across the tree
/// did not report (cut streams, aborts), so only a counted zero, or a null
/// that the transcripts' usage-less generations do not explain, is unmetered.
/// A generation that reported usage without the counter explains nothing.
fn metering(
    metrics: &SessionMetricsPayload,
    subject: &SubjectConfig,
    unreported: usize,
) -> Vec<HarnessViolation> {
    let totals = &metrics.totals;
    let uncounted = |counted: bool, reported: bool| !counted && (reported || unreported <= 1);
    let mut violations = [
        ("input", totals.input_tokens),
        ("output", totals.output_tokens),
    ]
    .into_iter()
    .filter(|(_, tokens)| uncounted(tokens.is_some_and(|tokens| tokens > 0), tokens.is_some()))
    .map(|(kind, tokens)| {
        let tokens = tokens.map_or_else(|| "no".to_string(), |tokens| tokens.to_string());
        violation(
            HarnessInvariant::UsageUnmetered,
            format!("the run counted {tokens} {kind} tokens"),
        )
    })
    .collect::<Vec<_>>();
    if subject.priced == Some(true)
        && uncounted(
            totals.cost_usd.is_some_and(|cost| cost > 0.0),
            totals.cost_usd.is_some(),
        )
    {
        violations.push(violation(
            HarnessInvariant::UsageUnmetered,
            format!(
                "the subject model is priced and the run counted a cost of {:?} USD",
                totals.cost_usd
            ),
        ));
    }
    violations
}

/// An aborted or failed generation can end before the provider reports usage,
/// and so can a stream cut mid-way: the Harness persists that partial with
/// `stop_reason` "end" and records a recovery right after it.
fn unreported_usage(session: &str, transcript: &Value) -> Option<HarnessViolation> {
    let entries = transcript
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let next = entries.iter().skip(1).map(Some).chain([None]);
    let unreported = entries
        .iter()
        .zip(next)
        .filter(|(_, next)| {
            !next.is_some_and(|next| text(next, "/custom/custom_type") == Some("recovery"))
        })
        .filter_map(|(entry, _)| Some((entry, entry.get("message")?)))
        .filter(|(_, message)| {
            text(message, "/role") == Some("assistant")
                && matches!(text(message, "/stop_reason"), Some("end" | "function_call"))
                && !message.get("usage").is_some_and(Value::is_object)
        })
        .map(|(entry, _)| text(entry, "/entry_id").unwrap_or("an unnamed entry"))
        .collect::<Vec<_>>();
    (!unreported.is_empty()).then(|| {
        violation(
            HarnessInvariant::UsageUnmetered,
            format!("{session}: {} ended without usage", unreported.join(", ")),
        )
    })
}

fn accounting(metrics: &SessionMetricsPayload) -> Vec<HarnessViolation> {
    let mut violations = Vec::new();
    // The Harness leaves a counter out of any sum more than one generation did
    // not report, so the sessions add up to the total only when all report one.
    let costs = metrics
        .by_session
        .iter()
        .map(|session| session.cost_usd)
        .collect::<Option<Vec<_>>>();
    if let (Some(total), Some(costs)) = (metrics.totals.cost_usd, costs) {
        let sum = costs.iter().sum::<f64>();
        if (sum - total).abs() > 1e-6 {
            violations.push(violation(
                HarnessInvariant::UsageMisaccounted,
                format!("the session costs add up to {sum} USD and the total is {total}"),
            ));
        }
    }
    for session in &metrics.by_session {
        let Some(context) = session.context.as_ref() else {
            continue;
        };
        let running = context.get("session_cost_usd").and_then(Value::as_f64);
        if let (Some(running), Some(cost)) = (running, session.cost_usd) {
            if (running - cost).abs() > 0.01 * cost {
                violations.push(violation(
                    HarnessInvariant::UsageMisaccounted,
                    format!(
                        "session {}: its running cost is {running} USD and its generations \
                         cost {cost}",
                        session.session_id
                    ),
                ));
            }
        }
        let count = |key| context.get(key).and_then(Value::as_u64);
        if let (Some(usable), Some(total), Some(free)) =
            (count("usable"), count("total"), count("free"))
        {
            if free != usable.saturating_sub(total) {
                violations.push(violation(
                    HarnessInvariant::UsageMisaccounted,
                    format!(
                        "session {}: free {free} of a {usable}-token window with {total} in use",
                        session.session_id
                    ),
                ));
            }
        }
    }
    violations
}

/// Root generations run on the subject's route. A child runs on the model its
/// spawn named, or on its parent's route when the spawn named none
/// (`harness/src/subagent.rs` `spawn_child`).
fn routing(
    metrics: &SessionMetricsPayload,
    subject: &SubjectConfig,
    transcripts: &[(&str, &Value)],
) -> Vec<HarnessViolation> {
    let root = metrics.root_session_id.as_str();
    let sessions = metrics
        .by_session
        .iter()
        .map(|session| (session.session_id.as_str(), session))
        .collect::<BTreeMap<_, _>>();
    let route_of = |session: &str| -> Option<(&str, &str)> {
        if session == root {
            return Some((&subject.provider, &subject.model));
        }
        match sessions.get(session)?.by_model.as_slice() {
            [usage] => Some((&usage.provider, &usage.model)),
            _ => None,
        }
    };
    let mut violations = Vec::new();
    for (session, transcript) in transcripts {
        if *session == root {
            let misrouted = generations(transcript)
                .filter(|(_, message)| {
                    text(message, "/provider") != Some(subject.provider.as_str())
                        || text(message, "/model") != Some(subject.model.as_str())
                })
                .map(|(entry, message)| {
                    let provider = text(message, "/provider").unwrap_or("?");
                    format!(
                        "{entry} ran on {provider}/{}",
                        text(message, "/model").unwrap_or("?")
                    )
                })
                .collect::<Vec<_>>();
            if !misrouted.is_empty() {
                violations.push(violation(
                    HarnessInvariant::ModelMisrouted,
                    format!(
                        "{root}: {} instead of {}/{}",
                        misrouted.join(", "),
                        subject.provider,
                        subject.model
                    ),
                ));
            }
        }
        let children = spawned_children(transcript);
        for invocation in crate::scenarios::common::function_invocations(transcript) {
            if invocation.call.function_id != "harness::spawn" {
                continue;
            }
            let Some(child) = invocation
                .call_id
                .as_deref()
                .and_then(|call| children.get(call))
                .and_then(|child| sessions.get(child.as_str()))
            else {
                continue;
            };
            let arguments = &invocation.call.arguments;
            let named = text(arguments, "/model");
            // A profile's route wins over the spawn's, and an in-turn spawn
            // that names no profile continues the subject's.
            if arguments.get("agent").is_some_and(|agent| !agent.is_null())
                || (subject.agent.is_some() && named.is_some())
            {
                continue;
            }
            let Some((provider, model)) = named
                .map(|model| (text(arguments, "/provider"), model))
                .or_else(|| route_of(session).map(|(provider, model)| (Some(provider), model)))
            else {
                continue;
            };
            let expected = provider.map_or_else(
                || model.to_string(),
                |provider| format!("{provider}/{model}"),
            );
            for usage in &child.by_model {
                if usage.model != model
                    || provider.is_some_and(|provider| provider != usage.provider)
                {
                    violations.push(violation(
                        HarnessInvariant::ModelMisrouted,
                        format!(
                            "{} ran on {}/{}, spawned for {expected}",
                            child.session_id, usage.provider, usage.model
                        ),
                    ));
                }
            }
        }
    }
    violations
}

/// The child session each successful, fresh spawn started, by call id. A
/// reused session kept generations from before this spawn.
fn spawned_children(transcript: &Value) -> BTreeMap<&str, String> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| text(message, "/role") == Some("function_result"))
        .filter(|message| message.get("is_error").and_then(Value::as_bool) != Some(true))
        .filter(|message| message.pointer("/details/reused").and_then(Value::as_bool) != Some(true))
        .filter_map(|message| {
            Some((
                text(message, "/function_call_id")?,
                text(message, "/details/child_session_id")?.to_string(),
            ))
        })
        .collect()
}

/// With the provider's native thinking switch off, the root reasons not at
/// all. shortcut: children are not held to it, since whether a child keeps
/// the parent's provider options depends on its route; check them once
/// verbose `harness::status` reports the options a session runs with.
fn reasoning(
    report: &E2eRunReport,
    metrics: &SessionMetricsPayload,
    subject: &SubjectConfig,
) -> Vec<HarnessViolation> {
    let disabled = subject
        .provider_options
        .as_ref()
        .and_then(|options| options.get(&subject.provider))
        .and_then(|options| text(options, "/thinking"))
        == Some("disabled");
    if !disabled {
        return Vec::new();
    }
    let root = metrics.root_session_id.as_str();
    let mut violations = Vec::new();
    if let Some(tokens) = metrics
        .by_session
        .iter()
        .find(|session| session.session_id == root)
        .and_then(|session| session.reasoning_tokens)
        .filter(|tokens| *tokens > 0)
    {
        violations.push(violation(
            HarnessInvariant::ReasoningWhileDisabled,
            format!("{root} counted {tokens} reasoning tokens with thinking disabled"),
        ));
    }
    let thinking = report
        .transcript
        .iter()
        .flat_map(generations)
        .filter(|(_, message)| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|block| matches!(text(block, "/type"), Some("thinking" | "redacted_thinking")))
        })
        .map(|(entry, _)| entry)
        .collect::<Vec<_>>();
    if !thinking.is_empty() {
        violations.push(violation(
            HarnessInvariant::ReasoningWhileDisabled,
            format!(
                "{root}: {} carried thinking with thinking disabled",
                thinking.join(", ")
            ),
        ));
    }
    violations
}

/// Assistant messages with their entry ids.
fn generations(transcript: &Value) -> impl Iterator<Item = (&str, &Value)> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let message = entry.get("message")?;
            (text(message, "/role") == Some("assistant")).then(|| {
                (
                    text(entry, "/entry_id").unwrap_or("an unnamed entry"),
                    message,
                )
            })
        })
}

fn violation(invariant: HarnessInvariant, detail: String) -> HarnessViolation {
    HarnessViolation { invariant, detail }
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::report::RunStatus;
    use crate::wire::Observed;

    fn subject() -> SubjectConfig {
        SubjectConfig {
            model: "m".into(),
            provider: "p".into(),
            agent: None,
            thinking_level: None,
            provider_options: None,
            priced: Some(true),
            context_window: None,
        }
    }

    fn assistant(entry_id: &str, cost: f64, content: Value) -> Value {
        json!({ "entry_id": entry_id, "message": {
            "role": "assistant", "stop_reason": "end", "model": "m", "provider": "p",
            "usage": { "input": 100, "output": 10, "reasoning": 0, "cost_usd": cost },
            "content": content,
        } })
    }

    fn spawn(call_id: &str, payload: Value) -> Value {
        json!([{ "type": "function_call", "id": call_id, "function_id": "agent_trigger",
                 "arguments": { "function": "harness::spawn", "payload": payload } }])
    }

    fn spawned(call_id: &str, child: &str) -> Value {
        json!({ "message": { "role": "function_result", "function_call_id": call_id,
                "function_id": "harness::spawn", "is_error": false,
                "details": { "child_session_id": child, "reused": false } } })
    }

    fn session(id: &str, parent: Option<&str>, turns: u64, cost: f64) -> Value {
        json!({
            "session_id": id, "parent_session_id": parent, "depth": u32::from(parent.is_some()),
            "turns": turns, "function_calls": 0, "function_call_errors": 0,
            "input_tokens": 100 * turns, "output_tokens": 10 * turns, "reasoning_tokens": 0,
            "cost_usd": cost,
            "by_model": [{ "provider": "p", "model": "m", "turns": turns }],
            "context": { "model": "m", "usable": 1_000, "total": 400, "free": 600,
                         "effective_max_output_tokens": 100, "session_cost_usd": cost },
        })
    }

    /// A passed run whose root spawned one child that inherits its model:
    /// every counter agrees.
    struct Run {
        root: Value,
        child: Value,
        metrics: Value,
    }

    impl Run {
        fn new() -> Self {
            Self {
                root: json!({ "messages": [
                    assistant("e1", 0.01, spawn("c1", json!({ "task": "count" }))),
                    spawned("c1", "child"),
                    assistant("e2", 0.02, json!([{ "type": "text", "text": "done" }])),
                ] }),
                child: json!({ "messages": [
                    assistant("e3", 0.03, json!([{ "type": "text", "text": "3" }])),
                ] }),
                metrics: json!({
                    "root_session_id": "root", "complete": true,
                    "totals": {
                        "sessions": 2, "turns": 3, "function_calls": 1, "function_call_errors": 0,
                        "input_tokens": 300, "output_tokens": 30, "reasoning_tokens": 0,
                        "cost_usd": 0.06,
                        "by_model": [{ "provider": "p", "model": "m", "turns": 3 }],
                    },
                    "by_session": [
                        session("root", None, 2, 0.03),
                        session("child", Some("root"), 1, 0.03),
                    ],
                }),
            }
        }

        fn set(mut self, target: &str, pointer: &str, value: Value) -> Self {
            let document = match target {
                "root" => &mut self.root,
                "child" => &mut self.child,
                _ => &mut self.metrics,
            };
            *document
                .pointer_mut(pointer)
                .unwrap_or_else(|| panic!("{target}{pointer} exists")) = value;
            self
        }

        fn check(self, subject: &SubjectConfig) -> Vec<HarnessViolation> {
            let mut report =
                E2eRunReport::new("run".into(), "a".into(), 1, "root".into(), "p".into());
            report.status = RunStatus::Passed;
            report.transcript = Some(self.root);
            report.child_transcripts = BTreeMap::from([("child".into(), self.child)]);
            report.metrics = Some(Observed::from_normalized(
                serde_json::from_value(self.metrics).expect("metrics decode"),
            ));
            violations(&report, subject)
        }
    }

    fn only(violations: &[HarnessViolation], invariant: HarnessInvariant, needle: &str) {
        assert_eq!(
            violations.iter().map(|v| v.invariant).collect::<Vec<_>>(),
            [invariant],
            "{violations:?}"
        );
        assert!(violations[0].detail.contains(needle), "{violations:?}");
    }

    #[test]
    fn a_metered_run_routed_where_it_was_sent_keeps_every_invariant() {
        assert_eq!(Run::new().check(&subject()), []);
    }

    #[test]
    fn uncounted_tokens_are_unmetered() {
        let violations = Run::new()
            .set("metrics", "/totals/output_tokens", json!(0))
            .check(&subject());

        only(
            &violations,
            HarnessInvariant::UsageUnmetered,
            "output tokens",
        );
    }

    #[test]
    fn totals_the_harness_nulled_over_two_cut_streams_are_not_unmetered() {
        let cut = |entry: &str| {
            json!([
                { "entry_id": entry, "message": {
                    "role": "assistant", "stop_reason": "end", "model": "m", "provider": "p",
                    "content": [] } },
                { "entry_id": format!("{entry}_recovering"), "custom": { "custom_type": "recovery" } },
            ])
        };
        let nulled = |cuts: &[&str]| {
            let mut messages = cuts
                .iter()
                .flat_map(|entry| cut(entry).as_array().unwrap().clone())
                .collect::<Vec<_>>();
            messages.extend(Run::new().root["messages"].as_array().unwrap().clone());
            Run::new()
                .set("root", "/messages", Value::Array(messages))
                .set("metrics", "/totals/input_tokens", Value::Null)
                .check(&subject())
        };

        assert_eq!(nulled(&["x1", "x2"]), []);
        only(
            &nulled(&["x1"]),
            HarnessInvariant::UsageUnmetered,
            "no input tokens",
        );
    }

    #[test]
    fn a_priced_run_without_a_cost_is_unmetered_and_an_unpriced_one_is_not() {
        let uncosted = || {
            Run::new()
                .set("metrics", "/totals/cost_usd", Value::Null)
                .set("metrics", "/by_session/0/cost_usd", Value::Null)
                .set("metrics", "/by_session/1/cost_usd", Value::Null)
                .set(
                    "metrics",
                    "/by_session/0/context/session_cost_usd",
                    Value::Null,
                )
                .set(
                    "metrics",
                    "/by_session/1/context/session_cost_usd",
                    Value::Null,
                )
        };
        let unpriced = SubjectConfig {
            priced: Some(false),
            ..subject()
        };

        only(
            &uncosted().check(&subject()),
            HarnessInvariant::UsageUnmetered,
            "cost",
        );
        assert_eq!(uncosted().check(&unpriced), []);
    }

    #[test]
    fn a_generation_without_usage_is_unmetered_unless_it_aborted_or_failed() {
        let violations = Run::new()
            .set("child", "/messages/0/message/usage", Value::Null)
            .check(&subject());
        only(&violations, HarnessInvariant::UsageUnmetered, "child: e3");

        for stop in ["aborted", "error"] {
            let violations = Run::new()
                .set("child", "/messages/0/message/usage", Value::Null)
                .set("child", "/messages/0/message/stop_reason", json!(stop))
                .check(&subject());
            assert_eq!(violations, [], "{stop}");
        }

        let cut = Run::new()
            .set(
                "child",
                "/messages",
                json!([
                    { "entry_id": "e3", "message": {
                        "role": "assistant", "stop_reason": "end", "model": "m", "provider": "p",
                        "content": [{ "type": "thinking", "text": "" }] } },
                    { "entry_id": "e_t1_transient_recovery_1_recovering",
                      "custom": { "custom_type": "recovery" } },
                    assistant("e4", 0.03, json!([{ "type": "text", "text": "3" }])),
                ]),
            )
            .check(&subject());
        assert_eq!(cut, []);
    }

    #[test]
    fn session_costs_that_do_not_add_up_to_the_total_are_misaccounted() {
        let violations = Run::new()
            .set("metrics", "/totals/cost_usd", json!(0.061))
            .check(&subject());

        only(&violations, HarnessInvariant::UsageMisaccounted, "add up");
    }

    #[test]
    fn a_running_session_cost_off_by_more_than_one_percent_is_misaccounted() {
        let within = Run::new()
            .set(
                "metrics",
                "/by_session/0/context/session_cost_usd",
                json!(0.0302),
            )
            .check(&subject());
        let off = Run::new()
            .set(
                "metrics",
                "/by_session/0/context/session_cost_usd",
                json!(0.0304),
            )
            .check(&subject());

        assert_eq!(within, []);
        only(&off, HarnessInvariant::UsageMisaccounted, "session root");
    }

    #[test]
    fn free_is_the_window_left_floored_at_zero() {
        let overfull = Run::new()
            .set("metrics", "/by_session/1/context/total", json!(1_200))
            .set("metrics", "/by_session/1/context/free", json!(0))
            .check(&subject());
        let skewed = Run::new()
            .set("metrics", "/by_session/1/context/free", json!(500))
            .check(&subject());

        assert_eq!(overfull, []);
        only(&skewed, HarnessInvariant::UsageMisaccounted, "free 500");
    }

    #[test]
    fn a_live_partial_snapshot_is_not_held_to_the_sums() {
        let violations = Run::new()
            .set("metrics", "/complete", json!(false))
            .set("metrics", "/totals/cost_usd", json!(0.061))
            .check(&subject());

        assert_eq!(violations, []);
    }

    #[test]
    fn a_root_generation_on_another_route_is_misrouted() {
        let violations = Run::new()
            .set("root", "/messages/2/message/provider", json!("other"))
            .check(&subject());

        only(
            &violations,
            HarnessInvariant::ModelMisrouted,
            "root: e2 ran on other/m",
        );
    }

    #[test]
    fn a_child_runs_on_its_spawn_model_or_else_its_parents() {
        let inherited = Run::new()
            .set("metrics", "/by_session/1/by_model/0/model", json!("n"))
            .check(&subject());
        only(
            &inherited,
            HarnessInvariant::ModelMisrouted,
            "child ran on p/n, spawned for p/m",
        );

        let named = |model: &str| {
            Run::new()
                .set(
                    "root",
                    "/messages/0/message/content/0/arguments/payload",
                    json!({ "model": "n" }),
                )
                .set("metrics", "/by_session/1/by_model/0/model", json!(model))
                .check(&subject())
        };
        assert_eq!(named("n"), []);
        only(
            &named("m"),
            HarnessInvariant::ModelMisrouted,
            "spawned for n",
        );
    }

    #[test]
    fn a_child_on_an_agent_profile_route_is_not_judged() {
        let violations = Run::new()
            .set(
                "root",
                "/messages/0/message/content/0/arguments/payload",
                json!({ "agent": "a" }),
            )
            .set("metrics", "/by_session/1/by_model/0/model", json!("n"))
            .check(&subject());

        assert_eq!(violations, []);
    }

    #[test]
    fn disabled_thinking_that_still_reasoned_is_flagged() {
        let disabled = SubjectConfig {
            provider_options: Some(json!({ "p": { "thinking": "disabled" } })),
            ..subject()
        };
        let reasoned = || {
            Run::new()
                .set(
                    "root",
                    "/messages/2/message/content",
                    json!([{ "type": "thinking", "text": "hm" }]),
                )
                .set("metrics", "/by_session/0/reasoning_tokens", json!(7))
        };

        let violations = reasoned().check(&disabled);
        assert_eq!(
            violations.iter().map(|v| v.invariant).collect::<Vec<_>>(),
            [HarnessInvariant::ReasoningWhileDisabled; 2],
            "{violations:?}"
        );
        assert_eq!(reasoned().check(&subject()), []);
        assert_eq!(Run::new().check(&disabled), []);
    }
}
