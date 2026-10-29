//! The plumbing the Harness contract probes share (`harness_contract_probe`,
//! `dispatch_pipeline`, `binding_contract_probe`, `turn_failure_surface`).
//! A probe is one direct call whose answer the Harness pins. `capture` runs
//! the probes against the live stack and records each answer it saw;
//! `evaluate` scores only that record, so the score can be replayed and a
//! unit test can feed it any record.

use std::time::Duration;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{AssessmentOutcome, AssessmentSpec};
use super::{
    ArtifactExpectation, CapturedDeliverable, DeliverableContract, ProvenanceEvidence,
    ScenarioObservation,
};

/// One probe: what it checked, whether the Harness answered as pinned, and
/// the answer itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Probe {
    pub id: String,
    pub passed: bool,
    pub observed: Value,
}

#[derive(Debug, Default)]
pub(super) struct Probes(pub Vec<Probe>);

impl Probes {
    pub fn check(&mut self, id: &str, passed: bool, observed: impl Serialize) {
        self.0.push(Probe {
            id: id.to_string(),
            passed,
            observed: serde_json::to_value(observed).unwrap_or(Value::Null),
        });
    }

    /// The deliverable `capture` returns: every probe, in the order it ran.
    pub fn deliverable(self, id: &str, kind: &str, session_id: &str) -> CapturedDeliverable {
        CapturedDeliverable {
            id: id.to_string(),
            kind: kind.to_string(),
            content: json!({ "probes": self.0 }).into(),
            invariants: Vec::new(),
            provenance: vec![ProvenanceEvidence {
                kind: "session".to_string(),
                source_id: session_id.to_string(),
                relation: "probed_harness_contract".to_string(),
            }],
        }
    }
}

pub(super) fn deliverable_contract(id: &str, kind: &str) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: id.to_string(),
            kind: kind.to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["probes"],
                "properties": { "probes": { "type": "array" } },
                "additionalProperties": true
            }),
            max_size_bytes: 262_144,
        }],
        invariants: Vec::new(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

/// The probes `capture` recorded, or `None` when it recorded none.
pub(super) fn captured(observation: &ScenarioObservation, id: &str) -> Option<Vec<Probe>> {
    let content = observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == id)?
        .content
        .as_json()?;
    serde_json::from_value(content.get("probes")?.clone()).ok()
}

/// Full points when every probe in `ids` ran and passed; the reason names the
/// ones that failed or never ran, with what each saw.
pub(super) fn award(spec: AssessmentSpec, probes: &[Probe], ids: &[&str]) -> AssessmentOutcome {
    let mut problems = Vec::new();
    for id in ids {
        match probes.iter().find(|probe| probe.id == *id) {
            Some(probe) if probe.passed => {}
            Some(probe) => problems.push(format!("{id}: {}", brief(&probe.observed))),
            None => problems.push(format!("{id}: never ran")),
        }
    }
    let passed = problems.is_empty();
    spec.full_or_zero(
        passed,
        if passed {
            format!("{} probe(s) answered as pinned", ids.len())
        } else {
            problems.join("; ")
        },
    )
}

fn brief(value: &Value) -> String {
    let text = value.to_string();
    match text.char_indices().nth(300) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// A function's answer: its value, or the error text the caller saw.
pub(super) async fn call(context: &E2eContext, function_id: &str, payload: Value) -> Answer {
    context
        .trigger_value(function_id, payload)
        .await
        .map_err(|error| format!("{error:#}"))
}

pub(super) type Answer = std::result::Result<Value, String>;

/// Whether the call was refused with an error naming `needle`.
pub(super) fn refused_with(answer: &Answer, needle: &str) -> bool {
    answer
        .as_ref()
        .err()
        .is_some_and(|error| error.contains(needle))
}

/// A `harness::send` the Harness should refuse. A send it accepts instead
/// started a turn nobody wants: stop it, so a regression costs no tokens.
pub(super) async fn refused_send(context: &E2eContext, request: Value) -> Answer {
    let answer = call(context, "harness::send", request).await;
    if let Ok(accepted) = &answer {
        if let Some(session_id) = accepted.get("session_id").and_then(Value::as_str) {
            let turn_id = accepted.get("turn_id").and_then(Value::as_str);
            let _ = context.stop_session(session_id, turn_id).await;
        }
    }
    answer
}

/// `harness::status` of `session_id` once its turn `turn_id` (any turn when
/// `None`) has settled: completed, failed or cancelled, with nothing queued.
pub(super) async fn settled_status(
    context: &E2eContext,
    session_id: &str,
    turn_id: Option<&str>,
    timeout: Duration,
) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = context
            .trigger_value(
                "harness::status",
                json!({ "session_id": session_id, "verbose": true }),
            )
            .await?;
        let settled = matches!(
            status.get("status").and_then(Value::as_str),
            Some("completed" | "failed" | "cancelled")
        ) && turn_id
            .is_none_or(|turn| status.get("turn_id").and_then(Value::as_str) == Some(turn))
            && status
                .get("queued")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty);
        if settled {
            return Ok(status);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("{session_id} did not settle within {timeout:?}: {status}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The custom entry `entry_id` of `transcript`, as `context.transcript` reads it.
pub(super) fn custom_entry<'a>(transcript: &'a Value, entry_id: &str) -> Option<&'a Value> {
    transcript
        .get("messages")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("entry_id").and_then(Value::as_str) == Some(entry_id))?
        .pointer("/custom/data")
}

/// The model and provider the run's subject is on, read from the first
/// generation or failure the transcript carries, so a probe session runs
/// on the subject's own route.
pub(super) fn subject_route(transcript: &Value) -> Option<(String, String)> {
    transcript
        .get("messages")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            let carrier = entry
                .get("message")
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
                .or_else(|| entry.pointer("/custom/data"))?;
            Some((
                carrier.get("model")?.as_str()?.to_string(),
                carrier.get("provider")?.as_str()?.to_string(),
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::EvaluationDimension;

    const SPEC: AssessmentSpec =
        AssessmentSpec::scored_in("spec", 10, "spec", EvaluationDimension::Deliverable);

    fn probe(id: &str, passed: bool) -> Probe {
        Probe {
            id: id.into(),
            passed,
            observed: json!({ "error": "x" }),
        }
    }

    #[test]
    fn a_criterion_pays_only_when_every_probe_it_names_ran_and_passed() {
        let probes = [probe("a", true), probe("b", false)];
        let scored = |ids: &[&str]| {
            let evaluation =
                super::super::assessment::build_evaluation([award(SPEC, &probes, ids)]);
            (
                evaluation.awards[0].awarded,
                evaluation.awards[0].reason.clone(),
            )
        };
        assert_eq!(scored(&["a"]).0, Some(10));
        let (awarded, reason) = scored(&["a", "b", "c"]);
        assert_eq!(awarded, Some(0));
        assert!(reason.contains("b: {\"error\":\"x\"}"), "{reason}");
        assert!(reason.contains("c: never ran"), "{reason}");
    }

    #[test]
    fn a_refusal_is_an_error_naming_the_pinned_text() {
        let refused: Answer = Err("handler error: harness/invalid_request: x is read-only".into());
        assert!(refused_with(&refused, "is read-only"));
        assert!(!refused_with(&refused, "frozen"));
        assert!(!refused_with(
            &Ok(json!({ "accepted": true })),
            "is read-only"
        ));
    }

    #[test]
    fn the_subject_route_comes_from_a_generation_or_a_failure() {
        let failed = json!({ "messages": [
            { "message": { "role": "user", "content": [] } },
            { "entry_id": "e_t_error", "custom": { "custom_type": "error",
                "data": { "model": "m", "provider": "p" } } },
        ] });
        assert_eq!(subject_route(&failed), Some(("m".into(), "p".into())));
        assert_eq!(
            custom_entry(&failed, "e_t_error").and_then(|data| data.get("model")),
            Some(&json!("m"))
        );
        assert_eq!(subject_route(&json!({ "messages": [] })), None);
    }
}
