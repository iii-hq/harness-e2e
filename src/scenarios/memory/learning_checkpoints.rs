//! Opt-in teacher-turn extraction evidence. No stored text or rule content is
//! persisted here; only configured canary labels and typed source provenance.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::cases::{Case, Check};
use super::runtime::{cursor_entry, cursor_target, memories_of, rules_of, teacher};
use super::score::Evidence;

/// One ordered completed teacher turn. Each label must have this many distinct
/// live matching-source memory ids; merged and split records are equally valid.
/// Repeated rows with the same id count once per label, including complementary rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExpectedCheckpoint {
    pub canaries: &'static [&'static str],
    pub holders_per_canary: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySource {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MemorySource>,
    pub live: bool,
    pub canaries: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureIssue {
    TranscriptUnavailable,
    TransportUnavailable,
    CursorTimeout,
    MalformedList,
}

/// Actual cursor and transcript entry identities are separate: no inferred
/// completion receipt. An infrastructure issue can never become a pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceCheckpoint {
    pub turn: usize,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_entry_id: Option<String>,
    pub waited_ms: u64,
    pub memories: Vec<MemoryFact>,
    /// Diagnostic only: rules cannot satisfy typed memory provenance.
    pub rule_canaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<CaptureIssue>,
}

pub fn config(case: &Case) -> Option<&'static [ExpectedCheckpoint]> {
    case.criteria
        .iter()
        .find_map(|criterion| match criterion.check {
            Check::LearningCheckpoints { expected } => Some(expected),
            _ => None,
        })
}

pub struct RawCheckpoint<'a> {
    pub transcript: &'a Value,
    pub cursor: &'a Value,
    pub list: &'a Value,
    pub rules: &'a Value,
}

/// Read independent RAW API replies; never a preview or a prefilled verdict.
/// Unknown-source text is used only to match labels, never copied to artifacts.
pub fn capture(
    turn: usize,
    session: &str,
    canaries: &BTreeMap<String, String>,
    raw: RawCheckpoint<'_>,
) -> EvidenceCheckpoint {
    let RawCheckpoint {
        transcript,
        cursor,
        list,
        rules,
    } = raw;
    let matches = |text: &str| {
        canaries
            .iter()
            .filter(|(_, value)| text.contains(value.as_str()))
            .map(|(label, _)| label.clone())
            .collect()
    };
    EvidenceCheckpoint {
        turn,
        session_id: session.into(),
        target_entry_id: cursor_target(transcript).map(str::to_string),
        cursor_entry_id: cursor_entry(cursor).map(str::to_string),
        waited_ms: 0,
        memories: memories_of(list)
            .into_iter()
            .map(|memory| MemoryFact {
                id: memory.id,
                source: memory
                    .session_id
                    .map(|session_id| MemorySource { session_id }),
                live: memory.live,
                canaries: matches(&memory.text),
            })
            .collect(),
        rule_canaries: matches(&rules_of(rules).into_values().collect::<Vec<_>>().join("\n")),
        issue: if !transcript["messages"].is_array() {
            Some(CaptureIssue::TranscriptUnavailable)
        } else if !list["memories"].is_array() || !rules["rules"].is_array() {
            Some(CaptureIssue::MalformedList)
        } else {
            None
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Configuration,
    CheckpointCount,
    CheckpointOrder,
    TeacherSession,
    MissingTarget,
    RepeatedTarget,
    StaleCursor,
    MissingCanary,
    UnexpectedCanary,
    HolderCount,
    CaptureUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub turn: Option<usize>,
}

pub fn failures(case: &Case, evidence: &Evidence) -> Vec<Failure> {
    let Some(expected) = config(case) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut add = |kind, turn| found.push(Failure { kind, turn });
    if expected.is_empty()
        || expected.len() != case.teacher.len()
        || expected.iter().any(|e| {
            e.canaries.is_empty()
                || e.canaries.iter().any(|label| label.is_empty())
                || e.holders_per_canary == 0
                || e.canaries.iter().collect::<BTreeSet<_>>().len() != e.canaries.len()
        })
        || case
            .criteria
            .iter()
            .filter(|c| matches!(c.check, Check::LearningCheckpoints { .. }))
            .count()
            != 1
    {
        add(FailureKind::Configuration, None);
    }
    let checkpoints = evidence.checkpoints.as_deref().unwrap_or_default();
    if checkpoints.len() != expected.len() {
        add(FailureKind::CheckpointCount, None);
    }
    // Bank identity determines the real attempt's teacher, not an owner or id
    // that happens to appear in stored text.
    let session = evidence.bank.strip_prefix("e2e-").map(teacher);
    let mut targets = BTreeSet::new();
    for (index, checkpoint) in checkpoints.iter().enumerate() {
        let turn = Some(index);
        if checkpoint.turn != index {
            add(FailureKind::CheckpointOrder, turn);
        }
        if session.as_deref() != Some(checkpoint.session_id.as_str()) {
            add(FailureKind::TeacherSession, turn);
        }
        match checkpoint
            .target_entry_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            None => add(FailureKind::MissingTarget, turn),
            Some(target) => {
                if !targets.insert(target) {
                    add(FailureKind::RepeatedTarget, turn);
                }
            }
        }
        if checkpoint.issue.is_some() {
            add(FailureKind::CaptureUnavailable, turn);
            // A timeout/transport failure is not evidence of completed extraction
            // lacking facts; never turn it into an objective model failure.
            continue;
        }
        if checkpoint.target_entry_id.is_none()
            || checkpoint.cursor_entry_id != checkpoint.target_entry_id
        {
            add(FailureKind::StaleCursor, turn);
            continue;
        }
        let Some(wanted) = expected.get(index) else {
            continue;
        };
        let eligible: Vec<_> = checkpoint
            .memories
            .iter()
            .filter(|memory| {
                memory.live
                    && !memory.id.is_empty()
                    && memory.source.as_ref().is_some_and(|source| {
                        source.session_id == checkpoint.session_id
                            && Some(source.session_id.as_str()) == session.as_deref()
                    })
            })
            .collect();
        for label in wanted.canaries {
            let holders: BTreeSet<_> = eligible
                .iter()
                .filter(|memory| memory.canaries.iter().any(|c| c.as_str() == *label))
                .map(|memory| &memory.id)
                .collect();
            if !evidence.canaries.contains_key(*label) || holders.is_empty() {
                add(FailureKind::MissingCanary, turn);
            }
            if holders.len() != wanted.holders_per_canary {
                add(FailureKind::HolderCount, turn);
            }
        }
        let configured: BTreeSet<_> = expected
            .iter()
            .flat_map(|e| e.canaries.iter().copied())
            .collect();
        if eligible.iter().any(|memory| {
            memory.canaries.iter().any(|label| {
                configured.contains(label.as_str()) && !wanted.canaries.contains(&label.as_str())
            })
        }) {
            add(FailureKind::UnexpectedCanary, turn);
        }
    }
    found
}

pub fn judge(case: &Case, evidence: &Evidence) -> (bool, String) {
    let failures = failures(case, evidence);
    (
        failures.is_empty(),
        format!(
            "learning checkpoints: {}",
            serde_json::to_string(&failures).expect("failures serialize")
        ),
    )
}

pub fn infrastructure_error(case: &Case, evidence: &Evidence) -> Option<String> {
    evidence
        .checkpoints
        .as_ref()
        .filter(|_| config(case).is_some())
        .and_then(|checkpoints| {
            checkpoints
                .iter()
                .find(|checkpoint| checkpoint.issue.is_some())
        })
        .map(|checkpoint| {
            format!(
                "teacher checkpoint {} capture unavailable: {:?}",
                checkpoint.turn, checkpoint.issue
            )
        })
}

pub fn has_objective_failure(case: &Case, evidence: &Evidence) -> bool {
    failures(case, evidence)
        .iter()
        .any(|failure| failure.kind != FailureKind::CaptureUnavailable)
}

#[cfg(test)]
mod tests {
    use super::super::cases::{canary, Criterion, CASES};
    use super::super::score::{self, PromptRecord, Snapshot};
    use super::*;
    use crate::report::{gate_completion, CompletionState, CriterionReport};
    use serde_json::json;

    const EXPECTED: &[ExpectedCheckpoint] = &[
        ExpectedCheckpoint {
            canaries: &["port_a"],
            holders_per_canary: 1,
        },
        ExpectedCheckpoint {
            canaries: &["port_a", "port_b"],
            holders_per_canary: 1,
        },
    ];
    // Offline contract fixture only; not a registered scenario or a new case.
    const CONTROL: Case = Case {
        id: "checkpoint_control",
        title: "Checkpoint control",
        summary: "Offline",
        protects: "Per-turn completion and source",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[],
        prompt: "Which ports?",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &["Port A is {port_a}.", "Port B is {port_b}."],
        teacher_reply: Some("noted"),
        changes: &[],
        criteria: &[Criterion {
            id: "checkpoints",
            weight: 100,
            gate: true,
            label: "Checkpoints",
            description: "Ordered completed teacher extractions",
            check: Check::LearningCheckpoints { expected: EXPECTED },
        }],
    };

    fn transcript(turn: usize) -> Value {
        let mut messages = Vec::new();
        for index in 0..=turn {
            let (label, port) = if index == 0 {
                ("port_a", "A")
            } else {
                ("port_b", "B")
            };
            messages.push(json!({"entry_id":format!("u{index}"), "message":{"role":"user", "content":[{"type":"text","text":format!("Port {port} is {}.",canary("control",label))}]}}));
            messages.push(json!({"entry_id":format!("a{index}"), "message":{"role":"assistant", "content":[{"type":"text","text":"noted"}]}}));
        }
        messages.push(json!({"entry_id":"notice", "custom":{"custom_type":"model_notice"}}));
        messages.push(json!({"entry_id":"tool", "message":{"role":"tool", "content":[]}}));
        json!({"messages": messages})
    }

    fn list(turn: usize) -> Value {
        let text = if turn == 0 {
            format!("Port A {}", canary("control", "port_a"))
        } else {
            format!(
                "Port A {}, Port B {}",
                canary("control", "port_a"),
                canary("control", "port_b")
            )
        };
        json!({"memories":[{"id":"one", "text":text, "source":{"session_id":"teach_control"}, "invalid_at":null}]})
    }

    fn captured(
        turn: usize,
        raw: &Value,
        cursor: &Value,
        list: &Value,
        rules: &Value,
    ) -> EvidenceCheckpoint {
        capture(
            turn,
            "teach_control",
            &[
                ("port_a".into(), canary("control", "port_a")),
                ("port_b".into(), canary("control", "port_b")),
            ]
            .into_iter()
            .collect(),
            RawCheckpoint {
                transcript: raw,
                cursor,
                list,
                rules,
            },
        )
    }

    fn evidence(first: EvidenceCheckpoint, second: EvidenceCheckpoint) -> Evidence {
        // Measured transcript is independent from teacher/list fixtures, and even
        // a correct final answer cannot excuse an extraction checkpoint defect.
        let raw = json!({"messages":[{"entry_id":"e_subject_0_assistant",
            "message":{"role":"assistant","content":[{"type":"text","text":format!("{} {}", canary("control","port_a"),canary("control","port_b"))}]},
            "origin":{"turn_id":"subject", "memory_bank":"e2e-control"}}]});
        let mut evidence = score::evidence(
            &CONTROL,
            "control",
            &raw,
            Snapshot {
                prompts: vec![PromptRecord {
                    turn_id: "subject".into(),
                    step: 0,
                    system_prompt: "helpful".into(),
                }],
                ..Snapshot::default()
            },
        );
        evidence.checkpoints = Some(vec![first, second]);
        evidence
    }

    fn positive() -> Evidence {
        evidence(
            captured(
                0,
                &transcript(0),
                &json!({"last_entry_id":"a0"}),
                &list(0),
                &json!({"rules":[]}),
            ),
            captured(
                1,
                &transcript(1),
                &json!({"value":{"last_entry_id":"a1"}}),
                &list(1),
                &json!({"rules":[]}),
            ),
        )
    }

    fn completion(evidence: &Evidence) -> CompletionState {
        let evaluation = score::evaluate(&CONTROL, evidence);
        let specs = score::criteria(&CONTROL);
        let award = &evaluation.awards[0];
        evaluation.completion.unwrap_or_else(|| {
            gate_completion(&[CriterionReport {
                id: specs[0].id.into(),
                description: None,
                label: None,
                possible: 100,
                awarded: award.awarded,
                reason: award.reason.clone(),
                gate: specs[0].gate,
                gate_minimum: specs[0].gate_minimum,
            }])
        })
    }

    fn fails(evidence: &Evidence, kind: FailureKind) {
        let failures = failures(&CONTROL, evidence);
        assert!(
            failures.iter().any(|f| f.kind == kind),
            "{kind:?}: {failures:?}"
        );
        let evaluation = score::evaluate(&CONTROL, evidence);
        assert_eq!(evaluation.awards[0].id, "checkpoints");
        assert_eq!(evaluation.awards[0].awarded, Some(0));
        assert!(evaluation.awards[0]
            .reason
            .contains(&serde_json::to_string(&kind).unwrap()));
        assert_eq!(completion(evidence), CompletionState::TaskIncomplete);
        if kind != FailureKind::CaptureUnavailable {
            assert_eq!(evaluation.infrastructure_error, None);
        }
    }

    #[test]
    fn direct_and_wrapped_cursors_complete_two_ordered_turns() {
        let evidence = positive();
        assert!(failures(&CONTROL, &evidence).is_empty());
        assert_eq!(
            score::evaluate(&CONTROL, &evidence).awards[0].awarded,
            Some(100)
        );
        assert_eq!(completion(&evidence), CompletionState::Completed);
        assert_eq!(
            score::evaluate(&CONTROL, &evidence).infrastructure_error,
            None
        );
        assert!(score::criteria(&CONTROL)[0].gate);
        assert_eq!(
            CONTROL.criteria[0].check.canaries().collect::<Vec<_>>(),
            ["port_a", "port_a", "port_b"]
        );
        assert_eq!(evidence.checkpoints.as_ref().unwrap()[1].memories.len(), 1);
    }

    #[test]
    fn newest_actual_assistant_noted_and_newest_user_are_targets() {
        let raw = transcript(1);
        let assistant = captured(
            1,
            &raw,
            &json!({"last_entry_id":"a1"}),
            &list(1),
            &json!({"rules":[]}),
        );
        assert_eq!(assistant.target_entry_id.as_deref(), Some("a1"));
        let mut user_raw = raw;
        user_raw["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"entry_id":"latest_user", "message":{"role":"user","content":[]}}));
        let user = captured(
            1,
            &user_raw,
            &json!({"value":{"last_entry_id":"latest_user"}}),
            &list(1),
            &json!({"rules":[]}),
        );
        assert_eq!(user.target_entry_id.as_deref(), Some("latest_user"));
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = user;
        assert!(judge(&CONTROL, &evidence).0);
    }

    #[test]
    fn missing_source_is_not_textual_provenance() {
        let mut raw = list(1);
        raw["memories"][0].as_object_mut().unwrap().remove("source");
        raw["memories"][0]["owner"] = "teach_control".into();
        raw["memories"][0]["id"] = "teach_control".into();
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::MissingCanary);
    }

    #[test]
    fn wrong_source_despite_text_and_rule_fails() {
        let mut raw = list(1);
        raw["memories"][0]["source"]["session_id"] = "another_teacher".into();
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[{"name":"learned","content":format!("{} {}",canary("control","port_a"),canary("control","port_b"))}]}),
        );
        fails(&evidence, FailureKind::MissingCanary);
    }

    #[test]
    fn rules_only_cannot_satisfy_typed_memory_facts() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &json!({"memories":[]}),
            &json!({"rules":[{"name":"learned","content":format!("{} {}",canary("control","port_a"),canary("control","port_b"))}]}),
        );
        fails(&evidence, FailureKind::MissingCanary);
        fails(&evidence, FailureKind::HolderCount);
    }

    #[test]
    fn future_turn_canary_cannot_arrive_early() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[0] = captured(
            0,
            &transcript(0),
            &json!({"last_entry_id":"a0"}),
            &list(1),
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::UnexpectedCanary);
    }

    #[test]
    fn completed_cursor_without_facts_is_objective_failure() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &json!({"memories":[]}),
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::MissingCanary);
    }

    #[test]
    fn stale_cursor_does_not_authorize_matching_facts() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a0"}),
            &list(1),
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::StaleCursor);
    }

    #[test]
    fn missing_checkpoints_cannot_be_skipped() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap().pop();
        fails(&evidence, FailureKind::CheckpointCount);
        evidence.checkpoints = None;
        fails(&evidence, FailureKind::CheckpointCount);
    }

    #[test]
    fn repeated_target_ids_fail_even_with_completed_cursor_and_facts() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(0),
            &json!({"last_entry_id":"a0"}),
            &list(1),
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::RepeatedTarget);
    }

    #[test]
    fn two_holders_fail_despite_correct_final_reply() {
        let mut raw = list(1);
        let mut second = raw["memories"][0].clone();
        second["id"] = "two".into();
        raw["memories"].as_array_mut().unwrap().push(second);
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::HolderCount);
    }

    #[test]
    fn same_holder_with_two_benign_facts_and_repeated_row_counts_once() {
        let mut raw = list(1);
        let repeated = raw["memories"][0].clone();
        raw["memories"].as_array_mut().unwrap().push(repeated);
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        assert!(judge(&CONTROL, &evidence).0);
    }

    #[test]
    fn superseded_holder_and_wrong_teacher_are_not_live_source() {
        let mut raw = list(1);
        raw["memories"][0]["invalid_at"] = "2026-01-01".into();
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::MissingCanary);
        evidence.checkpoints.as_mut().unwrap()[1].session_id = "other".into();
        fails(&evidence, FailureKind::TeacherSession);
    }

    #[test]
    fn a_second_real_teacher_session_cannot_satisfy_the_contract() {
        let canaries = [
            ("port_a".into(), canary("control", "port_a")),
            ("port_b".into(), canary("control", "port_b")),
        ]
        .into_iter()
        .collect();
        let mut raw = list(1);
        raw["memories"][0]["source"]["session_id"] = "teach_other".into();
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = capture(
            1,
            "teach_other",
            &canaries,
            RawCheckpoint {
                transcript: &transcript(1),
                cursor: &json!({"last_entry_id":"a1"}),
                list: &raw,
                rules: &json!({"rules":[]}),
            },
        );
        fails(&evidence, FailureKind::TeacherSession);
        fails(&evidence, FailureKind::MissingCanary);
    }

    #[test]
    fn reordered_or_missing_targets_fail() {
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap().swap(0, 1);
        fails(&evidence, FailureKind::CheckpointOrder);
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &json!({"messages":[]}),
            &json!({}),
            &list(1),
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::MissingTarget);
    }

    #[test]
    fn transport_timeout_and_malformed_capture_stay_infrastructure_nonpass() {
        for issue in [
            CaptureIssue::TransportUnavailable,
            CaptureIssue::CursorTimeout,
            CaptureIssue::MalformedList,
        ] {
            let mut evidence = positive();
            evidence.checkpoints.as_mut().unwrap()[0].issue = Some(issue);
            evidence.checkpoints.as_mut().unwrap().pop();
            let evaluation = score::evaluate(&CONTROL, &evidence);
            assert_eq!(evaluation.awards[0].awarded, Some(0));
            assert!(evaluation.infrastructure_error.is_some());
            assert_eq!(completion(&evidence), CompletionState::TaskIncomplete);
            assert!(failures(&CONTROL, &evidence)
                .iter()
                .any(|f| f.kind == FailureKind::CaptureUnavailable));
        }
        assert_eq!(
            captured(
                0,
                &transcript(0),
                &json!({}),
                &json!({}),
                &json!({"rules":[]})
            )
            .issue,
            Some(CaptureIssue::MalformedList)
        );
        assert_eq!(
            captured(0, &Value::Null, &json!({}), &list(0), &json!({"rules":[]})).issue,
            Some(CaptureIssue::TranscriptUnavailable)
        );
    }

    #[test]
    fn split_holders_pass_and_shared_label_duplicate_fails_per_canary() {
        let a = canary("control", "port_a");
        let b = canary("control", "port_b");
        let mut raw = json!({"memories":[
            {"id":"a", "text":a, "source":{"session_id":"teach_control"}, "invalid_at":null},
            {"id":"b", "text":b, "source":{"session_id":"teach_control"}, "invalid_at":null}
        ]});
        let mut evidence = positive();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        assert!(judge(&CONTROL, &evidence).0);
        assert_eq!(completion(&evidence), CompletionState::Completed);
        // Still two union holders, but port_a now has two distinct live ids.
        raw["memories"][1]["text"] = format!("{a} {b}").into();
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        fails(&evidence, FailureKind::HolderCount);
    }

    #[test]
    fn holder_count_is_configurable_per_label_and_rejects_empty_labels() {
        const COUNTED: Case = Case {
            criteria: &[Criterion {
                check: Check::LearningCheckpoints {
                    expected: &[
                        ExpectedCheckpoint {
                            canaries: &["port_a"],
                            holders_per_canary: 1,
                        },
                        ExpectedCheckpoint {
                            canaries: &["port_a", "port_b"],
                            holders_per_canary: 2,
                        },
                    ],
                },
                ..CONTROL.criteria[0]
            }],
            ..CONTROL
        };
        let mut raw = list(1);
        let mut other = raw["memories"][0].clone();
        other["id"] = "second".into();
        raw["memories"].as_array_mut().unwrap().push(other);
        let mut evidence = positive();
        assert!(!judge(&COUNTED, &evidence).0);
        evidence.checkpoints.as_mut().unwrap()[1] = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[]}),
        );
        assert!(judge(&COUNTED, &evidence).0);
        const INVALID: Case = Case {
            criteria: &[Criterion {
                check: Check::LearningCheckpoints {
                    expected: &[
                        ExpectedCheckpoint {
                            canaries: &[""],
                            holders_per_canary: 1,
                        },
                        ExpectedCheckpoint {
                            canaries: &["port_a", "port_b"],
                            holders_per_canary: 0,
                        },
                    ],
                },
                ..CONTROL.criteria[0]
            }],
            ..CONTROL
        };
        assert!(failures(&INVALID, &evidence)
            .iter()
            .any(|f| f.kind == FailureKind::Configuration));
    }

    #[test]
    fn opt_in_only_and_artifacts_never_copy_stored_text() {
        let opted_in = super::super::incremental_cases::INCREMENTAL_LONG_RECALL.id;
        assert_eq!(
            CASES
                .iter()
                .filter(|case| config(case).is_some())
                .map(|case| case.id)
                .collect::<Vec<_>>(),
            [opted_in]
        );
        for case in CASES.iter().filter(|case| case.id != opted_in) {
            assert!(config(case).is_none());
            let evidence =
                score::evidence(case, "legacy", &json!({"messages":[]}), Snapshot::default());
            assert!(serde_json::to_value(evidence)
                .unwrap()
                .get("checkpoints")
                .is_none());
        }
        let mut raw = list(1);
        raw["memories"][0]["text"] =
            format!("PRIVATE-FOREIGN-TEXT {}", canary("control", "port_a")).into();
        raw["memories"][0]["source"]["session_id"] = "foreign".into();
        let captured = captured(
            1,
            &transcript(1),
            &json!({"last_entry_id":"a1"}),
            &raw,
            &json!({"rules":[{"name":"secret","content":"PRIVATE-RULE"}]}),
        );
        let serialized = serde_json::to_string(&captured).unwrap();
        assert!(!serialized.contains("PRIVATE"));
        assert!(!serialized.contains(&canary("control", "port_a")));
        assert_eq!(
            serde_json::from_str::<EvidenceCheckpoint>(&serialized).unwrap(),
            captured
        );
    }
}
