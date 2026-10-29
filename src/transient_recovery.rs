//! Transient recovery across the session tree. The Harness writes a
//! `e_<turn>_transient_recovery_<n>_recovering` row for each resume after a
//! stream cut, and `..._<n>_recovered` once a generation after resume `n`
//! succeeds (`harness/src/ids.rs`, `turn_loop.rs`). Neither `harness::metrics`
//! nor the root's status counts a child's resumes, so these rows, read from
//! every session's transcript, are the count.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::E2eContext;
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::wire::SessionMetricsPayload;

/// One turn's recovery rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnRecovery {
    pub session_id: String,
    pub turn_id: String,
    /// The attempt of each `recovering` row: one per transient resume.
    pub recovering: Vec<u32>,
    /// The attempt of each `recovered` row.
    pub recovered: Vec<u32>,
    /// The turn's `max_transient_resumes`, as its rows carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    /// The turn failed or was stopped (`e_<turn>_error`, `e_<turn>_stopped`),
    /// so its last resume had no generation left to recover.
    pub ended_without_result: bool,
}

/// The root's rows and every other session's in `metrics.by_session`.
pub async fn collect(
    context: &E2eContext,
    root_session_id: &str,
    root_transcript: &Value,
    metrics: &SessionMetricsPayload,
) -> Result<Vec<TurnRecovery>> {
    let mut recoveries = turns(root_session_id, root_transcript);
    for session in &metrics.by_session {
        if session.session_id != root_session_id {
            let transcript = context.transcript(&session.session_id).await?;
            recoveries.extend(turns(&session.session_id, &transcript));
        }
    }
    Ok(recoveries)
}

/// The turns of one transcript that wrote recovery rows.
pub fn turns(session_id: &str, transcript: &Value) -> Vec<TurnRecovery> {
    let entries = transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let ids = entries
        .iter()
        .filter_map(|entry| entry.get("entry_id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let mut turns = BTreeMap::<&str, TurnRecovery>::new();
    for entry in entries {
        let Some((turn_id, attempt, outcome)) = entry
            .get("entry_id")
            .and_then(Value::as_str)
            .and_then(|id| id.strip_prefix("e_"))
            .and_then(|id| id.split_once("_transient_recovery_"))
            .and_then(|(turn_id, rest)| {
                let (attempt, outcome) = rest.split_once('_')?;
                Some((turn_id, attempt.parse::<u32>().ok()?, outcome))
            })
        else {
            continue;
        };
        let turn = turns.entry(turn_id).or_insert_with(|| TurnRecovery {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            recovering: Vec::new(),
            recovered: Vec::new(),
            max_attempts: None,
            ended_without_result: ids.contains(format!("e_{turn_id}_error").as_str())
                || ids.contains(format!("e_{turn_id}_stopped").as_str()),
        });
        match outcome {
            "recovering" => turn.recovering.push(attempt),
            "recovered" => turn.recovered.push(attempt),
            _ => {}
        }
        if let Some(max) = entry
            .pointer("/custom/data/max_attempts")
            .and_then(Value::as_u64)
        {
            turn.max_attempts = u32::try_from(max).ok();
        }
    }
    turns.into_values().collect()
}

/// Transient resumes over the turns, every session's included.
pub fn resumes(turns: &[TurnRecovery]) -> u64 {
    turns.iter().map(|turn| turn.recovering.len() as u64).sum()
}

/// A resume is recovered by its own `recovered` row or a later attempt's:
/// back-to-back cuts write `recovering` 1 and 2, then only `recovered` 2.
pub fn violations(turns: &[TurnRecovery]) -> Vec<HarnessViolation> {
    let mut violations = Vec::new();
    for turn in turns {
        let recovered = turn.recovered.iter().max();
        for &attempt in &turn.recovering {
            let at = || {
                format!(
                    "resume {attempt} of turn {} in session {}",
                    turn.turn_id, turn.session_id
                )
            };
            if !turn.ended_without_result && recovered.is_none_or(|last| attempt > *last) {
                violations.push(HarnessViolation {
                    invariant: HarnessInvariant::UnrecoveredTransientResume,
                    detail: format!("{} has no recovered row", at()),
                });
            }
            if let Some(max) = turn.max_attempts.filter(|max| attempt > *max) {
                violations.push(HarnessViolation {
                    invariant: HarnessInvariant::TransientResumesOverLimit,
                    detail: format!("{} exceeds max_transient_resumes {max}", at()),
                });
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(turn: &str, attempt: u32, outcome: &str) -> Value {
        json!({
            "entry_id": format!("e_{turn}_transient_recovery_{attempt}_{outcome}"),
            "custom": { "custom_type": "recovery", "data": {
                "status": outcome, "attempt": attempt, "max_attempts": 3,
            } },
        })
    }

    fn transcript(rows: Vec<Value>) -> Value {
        json!({ "messages": rows })
    }

    fn invariants(violations: &[HarnessViolation]) -> Vec<HarnessInvariant> {
        violations.iter().map(|v| v.invariant).collect()
    }

    #[test]
    fn rows_group_by_turn_and_count_one_resume_per_recovering_row() {
        let turns = turns(
            "child",
            &transcript(vec![
                row("t_a", 1, "recovering"),
                json!({ "entry_id": "e_t_a_transient_resume_1", "message": { "role": "user" } }),
                row("t_a", 2, "recovering"),
                row("t_a", 2, "recovered"),
                row("t_b", 1, "recovering"),
                row("t_b", 1, "recovered"),
                json!({ "entry_id": "e_t_c_1_assistant", "message": { "role": "assistant" } }),
            ]),
        );

        assert_eq!(
            turns,
            [
                TurnRecovery {
                    session_id: "child".into(),
                    turn_id: "t_a".into(),
                    recovering: vec![1, 2],
                    recovered: vec![2],
                    max_attempts: Some(3),
                    ended_without_result: false,
                },
                TurnRecovery {
                    session_id: "child".into(),
                    turn_id: "t_b".into(),
                    recovering: vec![1],
                    recovered: vec![1],
                    max_attempts: Some(3),
                    ended_without_result: false,
                },
            ]
        );
        assert_eq!(resumes(&turns), 3);
        assert!(violations(&turns).is_empty());
    }

    #[test]
    fn a_resume_no_later_generation_recovered_is_flagged() {
        let turns = turns(
            "root",
            &transcript(vec![
                row("t_a", 1, "recovering"),
                row("t_a", 1, "recovered"),
                row("t_a", 2, "recovering"),
            ]),
        );

        let violations = violations(&turns);

        assert_eq!(
            invariants(&violations),
            [HarnessInvariant::UnrecoveredTransientResume]
        );
        assert!(violations[0].detail.contains("resume 2 of turn t_a"));
        assert!(violations[0].detail.contains("session root"));
    }

    #[test]
    fn a_turn_that_failed_or_stopped_leaves_its_last_resume_open() {
        for end in ["e_t_a_error", "e_t_a_stopped"] {
            let turns = turns(
                "child",
                &transcript(vec![
                    row("t_a", 1, "recovering"),
                    json!({ "entry_id": end, "custom": { "custom_type": "error" } }),
                ]),
            );

            assert!(turns[0].ended_without_result);
            assert!(violations(&turns).is_empty());
        }
    }

    #[test]
    fn a_resume_past_max_transient_resumes_is_flagged() {
        let turns = turns(
            "root",
            &transcript(vec![
                row("t_a", 3, "recovering"),
                row("t_a", 4, "recovering"),
                row("t_a", 4, "recovered"),
            ]),
        );

        let violations = violations(&turns);

        assert_eq!(
            invariants(&violations),
            [HarnessInvariant::TransientResumesOverLimit]
        );
        assert!(violations[0].detail.contains("resume 4 of turn t_a"));
    }
}
