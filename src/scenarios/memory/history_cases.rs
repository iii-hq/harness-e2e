//! Selection between two planted records that label themselves: the original
//! plan and the current one. Not correction handling: no teacher turn, no
//! supersede link and no timestamp says which plan came first; their text does.

use super::cases::{Case, Check, Criterion, Seed};

/// The record labelled current now, and both labelled records on request.
pub const CURRENT_AND_HISTORY: Case = Case {
    id: "memory_current_and_history",
    title: "The labelled current plan, then both plans",
    summary: "Two planted records of an analytics export label themselves as the original and the current plan; the first question asks for the current plan only and the follow-up for both.",
    protects: "Recall picks between two records by the plan each one says it holds. If it fails, a question about the current plan gets the record labelled original, or the follow-up loses one of the two. Both records are planted with their labels, so this does not test how memory learns a correction.",
    baseline: false,
    distractors: false,
    memories: &[
        Seed {
            text: "Original-plan record for the analytics export: the user originally stated that it runs every Tuesday at 09:00 and writes to the bucket export-{old}. This record describes the original plan, not the current schedule.",
            group: "export-original-plan",
            needed_on: &[1],
            related_on: &[],
            entities: &["analytics-export-original-plan"],
        },
        Seed {
            text: "Corrected-plan record for the analytics export: the user explicitly corrected the schedule; it now runs every Thursday at 18:00 and writes to the bucket export-{new}. This record describes the current plan, not the original schedule.",
            group: "export-corrected-plan",
            needed_on: &[0, 1],
            related_on: &[],
            entities: &["analytics-export-corrected-plan"],
        },
    ],
    rules: &[],
    prompt: "What is the current corrected schedule for the analytics export, and which bucket does it write to? Give only the current plan, not the original one.",
    followups: &["Now give both the original user-stated analytics export schedule and bucket, and the user's explicitly corrected schedule and bucket. Clearly label which plan is original and which is current."],
    compact_before: None,
    preview_first: &[],
    teacher: &[],
    teacher_reply: None,
    changes: &[],
    criteria: &[
        Criterion {
            id: "needed_plans_delivered",
            weight: 20,
            gate: true,
            description: "Was the corrected plan available on both exchanges and the original plan available on the history exchange?",
            label: "Each requested plan reaches its exchange",
            check: Check::NeededInjected { exchange: None },
        },
        Criterion {
            id: "only_requested_plans_delivered",
            weight: 15,
            gate: true,
            description: "Was the original plan withheld on the current-only exchange, with no unknown or irrelevant delivery?",
            label: "Only requested plans delivered",
            check: Check::NothingIrrelevant,
        },
        Criterion {
            id: "no_duplicate_plans",
            weight: 10,
            gate: true,
            description: "Was each independent plan delivered at most once while visible, without collapsing original and corrected values together?",
            label: "No plan delivered twice",
            check: Check::NoDuplicates,
        },
        Criterion {
            id: "strict_delivery",
            weight: 15,
            gate: true,
            description: "Do captured hook notices satisfy the complete mechanical delivery contract for both exchanges?",
            label: "Recorded delivery meets the contract",
            check: Check::StrictDelivery,
        },
        Criterion {
            id: "current_answer_has_new",
            weight: 10,
            gate: true,
            description: "Does the current-only answer contain the run's corrected bucket value?",
            label: "Current answer uses the correction",
            check: Check::AnswerContains {
                exchange: Some(0),
                canary: "new",
            },
        },
        Criterion {
            id: "current_answer_omits_old",
            weight: 10,
            gate: true,
            description: "Does the current-only answer omit the run's original bucket value?",
            label: "Current answer omits the original",
            check: Check::AnswerOmits {
                exchange: Some(0),
                canary: "old",
            },
        },
        Criterion {
            id: "history_answer_has_old",
            weight: 10,
            gate: true,
            description: "Does the explicit history answer contain the run's original bucket value?",
            label: "History answer includes the original",
            check: Check::AnswerContains {
                exchange: Some(1),
                canary: "old",
            },
        },
        Criterion {
            id: "history_answer_has_new",
            weight: 10,
            gate: true,
            description: "Does the explicit history answer also contain the run's corrected bucket value?",
            label: "History answer includes the correction",
            check: Check::AnswerContains {
                exchange: Some(1),
                canary: "new",
            },
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::super::cases::{self, canary};
    use super::super::score::{self, Evidence, PromptRecord, Snapshot};
    use super::super::strict::{self, Failure, FailureKind};
    use super::*;
    use crate::report::{gate_completion, CompletionState, CriterionReport};
    use serde_json::{json, Value};

    const RUN: &str = "history-control";
    const BANK: &str = "e2e-history-control";
    const PROMPT: &str = "Helpful.\n\n# Memory — bank: e2e-history-control\nAutomatic memory.";

    // Authored independently of the case's seed list, permissions and rendering.
    // These are raw stored text fixtures, not timestamp or source metadata claims.
    fn original() -> String {
        format!("Original-plan record for the analytics export: the user originally stated that it runs every Tuesday at 09:00 and writes to the bucket export-{}. This record describes the original plan, not the current schedule.", canary(RUN, "old"))
    }

    fn corrected() -> String {
        format!("Corrected-plan record for the analytics export: the user explicitly corrected the schedule; it now runs every Thursday at 18:00 and writes to the bucket export-{}. This record describes the current plan, not the original schedule.", canary(RUN, "new"))
    }

    fn notice(turn: &str, lines: &[&str]) -> Value {
        let lines: String = lines.iter().map(|line| format!("- {line}\n")).collect();
        json!({
            "entry_id": format!("e_{turn}_0_notice_0"),
            "custom": {"custom_type": "model_notice", "data": {
                "kind": "hook",
                "text": format!("<memory bank=\"{BANK}\">\nRelevant remembered memories:\n{lines}</memory>")
            }}
        })
    }

    fn assistant(turn: &str, count: usize, answer: &str) -> Value {
        json!({
            "entry_id": format!("e_{turn}_0_assistant"),
            "message": {"role": "assistant", "content": [{"type": "text", "text": answer}]},
            "origin": {"turn_id": turn, "memory_bank": BANK, "memory_recalled": count,
                "req": {"system_sha": score::system_sha(PROMPT)}}
        })
    }

    fn raw(current: &[&str], history: &[&str]) -> Value {
        json!({"messages": [
            notice("current", current),
            assistant("current", current.len(), &format!("Current: Thursday at 18:00, export-{}.", canary(RUN, "new"))),
            notice("history", history),
            assistant("history", history.len(), &format!("Original: Tuesday at 09:00, export-{}. Corrected: Thursday at 18:00, export-{}.", canary(RUN, "old"), canary(RUN, "new")))
        ]})
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            prompts: ["current", "history"]
                .into_iter()
                .map(|turn| PromptRecord {
                    turn_id: turn.into(),
                    step: 0,
                    system_prompt: PROMPT.into(),
                })
                .collect(),
            stored: vec![Some(original()), Some(corrected())],
            ..Snapshot::default()
        }
    }

    fn capture(raw: &Value) -> Evidence {
        score::evidence(&CURRENT_AND_HISTORY, RUN, raw, snapshot())
    }

    fn completion(evidence: &Evidence) -> CompletionState {
        let evaluation = score::evaluate(&CURRENT_AND_HISTORY, evidence);
        let reports: Vec<_> = score::criteria(&CURRENT_AND_HISTORY)
            .iter()
            .map(|criterion| {
                let award = evaluation
                    .awards
                    .iter()
                    .find(|a| a.id == criterion.id)
                    .unwrap();
                CriterionReport {
                    id: criterion.id.into(),
                    description: None,
                    label: None,
                    possible: criterion.weight,
                    awarded: award.awarded,
                    reason: award.reason.clone(),
                    gate: criterion.gate,
                    gate_minimum: criterion.gate_minimum,
                }
            })
            .collect();
        evaluation
            .completion
            .unwrap_or_else(|| gate_completion(&reports))
    }

    fn assert_gate_failed(evidence: &Evidence, id: &str) {
        let evaluation = score::evaluate(&CURRENT_AND_HISTORY, evidence);
        let award = evaluation.awards.iter().find(|a| a.id == id).unwrap();
        assert_eq!(award.awarded, Some(0), "{id}: {}", award.reason);
        assert_eq!(completion(evidence), CompletionState::TaskIncomplete);
        assert_eq!(evaluation.infrastructure_error, None);
    }

    fn assert_delivery_failure(raw: &Value, kind: FailureKind, exchange: usize, gate: &str) {
        let evidence = capture(raw);
        let failures = strict::failures(&CURRENT_AND_HISTORY, &evidence);
        assert!(
            failures.contains(&Failure {
                kind,
                exchange: Some(exchange)
            }),
            "{failures:?}"
        );
        assert_gate_failed(&evidence, "strict_delivery");
        assert_gate_failed(&evidence, gate);
    }

    #[test]
    fn capture_controls_do_not_confuse_observer_loss_with_delivery_defects() {
        super::super::attribute_cases::assert_capture_controls(
            &CURRENT_AND_HISTORY,
            RUN,
            &raw(&[&corrected()], &[&original()]),
            snapshot,
        );
        let evidence = capture(&raw(&[], &[]));
        assert_gate_failed(&evidence, "strict_delivery");
        assert_gate_failed(&evidence, "needed_plans_delivered");
    }

    #[test]
    fn contract_keeps_two_independent_unretired_plans_in_save_order() {
        let case = &CURRENT_AND_HISTORY;
        assert_eq!(case.id, "memory_current_and_history");
        assert_eq!(case.memories.len(), 2);
        assert_eq!(
            case.memories
                .iter()
                .map(|s| (s.group, s.needed_on, s.related_on))
                .collect::<Vec<_>>(),
            vec![
                ("export-original-plan", &[1][..], &[][..]),
                ("export-corrected-plan", &[0, 1][..], &[][..]),
            ]
        );
        assert_eq!(
            case.memories[0].entities,
            &["analytics-export-original-plan"]
        );
        assert_eq!(
            case.memories[1].entities,
            &["analytics-export-corrected-plan"]
        );
        assert!(case.memories[0]
            .entities
            .iter()
            .all(|e| !case.memories[1].entities.contains(e)));
        assert_eq!(
            cases::planted_memories(case, RUN),
            vec![original(), corrected()]
        );
        assert!(!case.baseline && !case.distractors);
        assert!(case.rules.is_empty() && case.changes.is_empty() && case.teacher.is_empty());
        assert!(case.teacher_reply.is_none() && case.compact_before.is_none());
        assert!(case.preview_first.is_empty());
        assert_eq!(case.followups.len(), 1);
        assert!(case.prompt.contains("only the current plan"));
        assert!(case.followups[0].contains("both the original"));
        assert!(case.memories[0].text.contains("Tuesday at 09:00"));
        assert!(case.memories[1].text.contains("user explicitly corrected"));
        assert!(case.memories[1].text.contains("Thursday at 18:00"));
        assert_eq!(case.criteria.len(), 8);
        assert_eq!(
            case.criteria
                .iter()
                .map(|c| u16::from(c.weight))
                .sum::<u16>(),
            100
        );
        assert!(case.criteria.iter().all(|c| c.gate && c.weight > 0));
        let checks: Vec<_> = case.criteria.iter().map(|c| c.check).collect();
        assert_eq!(
            checks,
            vec![
                Check::NeededInjected { exchange: None },
                Check::NothingIrrelevant,
                Check::NoDuplicates,
                Check::StrictDelivery,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "new"
                },
                Check::AnswerOmits {
                    exchange: Some(0),
                    canary: "old"
                },
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: "old"
                },
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: "new"
                },
            ]
        );
    }

    #[test]
    fn prompts_never_supply_the_run_scoped_bucket_canaries() {
        for label in ["old", "new"] {
            assert_ne!(canary(RUN, label), canary("history-other-run", label));
            for prompt in std::iter::once(CURRENT_AND_HISTORY.prompt)
                .chain(CURRENT_AND_HISTORY.followups.iter().copied())
            {
                assert!(!prompt.contains(&format!("{{{label}}}")));
                assert!(!prompt.contains(&canary(RUN, label)));
                assert!(!prompt.contains("export-"));
                assert!(!prompt.contains("Tuesday") && !prompt.contains("Thursday"));
            }
        }
        assert_ne!(canary(RUN, "old"), canary(RUN, "new"));
    }

    #[test]
    fn independent_current_then_history_fixture_passes_without_collapsing_plans() {
        let evidence = capture(&raw(&[&corrected()], &[&original()]));
        assert_eq!(strict::failures(&CURRENT_AND_HISTORY, &evidence), vec![]);
        assert_eq!(score::audit(&evidence).missing, vec![]);
        assert_eq!(evidence.observed.blocks[0].lines, vec![corrected()]);
        assert_eq!(evidence.observed.blocks[1].lines, vec![original()]);
        assert_eq!(evidence.observed.exchanges[0].memory_recalled, 1);
        assert_eq!(evidence.observed.exchanges[1].memory_recalled, 1);
        assert!(evidence
            .prompts
            .iter()
            .all(|p| p.recorded && p.sha_matches == Some(true)));
        let evaluation = score::evaluate(&CURRENT_AND_HISTORY, &evidence);
        for criterion in CURRENT_AND_HISTORY.criteria {
            assert_eq!(
                evaluation
                    .awards
                    .iter()
                    .find(|a| a.id == criterion.id)
                    .unwrap()
                    .awarded,
                Some(criterion.weight)
            );
        }
        assert_eq!(evaluation.infrastructure_error, None);
        assert_eq!(completion(&evidence), CompletionState::Completed);
    }

    #[test]
    fn missing_original_on_history_is_delivery_failure_even_if_answer_guesses_it() {
        let transcript = raw(&[&corrected()], &[]);
        assert_delivery_failure(
            &transcript,
            FailureKind::MissingGroup,
            1,
            "needed_plans_delivered",
        );
        // Losing prompt capture must not turn a known missing history into an
        // infrastructure excuse; the raw transcript still proves the omission.
        let evidence = score::evidence(&CURRENT_AND_HISTORY, RUN, &transcript, Snapshot::default());
        assert_gate_failed(&evidence, "strict_delivery");
    }

    #[test]
    fn original_on_current_fails_even_when_the_answer_omits_it() {
        assert_delivery_failure(
            &raw(&[&corrected(), &original()], &[]),
            FailureKind::ExtraGroup,
            0,
            "only_requested_plans_delivered",
        );
    }

    #[test]
    fn missing_correction_fails_on_current_and_when_absent_entirely() {
        for history in [vec![original(), corrected()], vec![original()]] {
            let lines: Vec<_> = history.iter().map(String::as_str).collect();
            assert_delivery_failure(
                &raw(&[], &lines),
                FailureKind::MissingGroup,
                0,
                "needed_plans_delivered",
            );
        }
        let evidence = capture(&raw(&[], &[&original()]));
        assert!(
            strict::failures(&CURRENT_AND_HISTORY, &evidence).contains(&Failure {
                kind: FailureKind::MissingGroup,
                exchange: Some(1),
            })
        );
    }

    #[test]
    fn duplicate_delivery_fails_within_history_or_across_visible_exchanges() {
        for (current, history, exchange) in [
            (vec![corrected(), corrected()], vec![original()], 0),
            (vec![corrected()], vec![original(), original()], 1),
            (vec![corrected()], vec![original(), corrected()], 1),
        ] {
            let current: Vec<_> = current.iter().map(String::as_str).collect();
            let history: Vec<_> = history.iter().map(String::as_str).collect();
            assert_delivery_failure(
                &raw(&current, &history),
                FailureKind::DuplicateGroup,
                exchange,
                "no_duplicate_plans",
            );
        }
    }

    #[test]
    fn answer_checks_are_exchange_specific_and_cannot_be_satisfied_by_delivery_alone() {
        for (entry, answer, gate) in [
            (
                1,
                format!(
                    "Current bucket export-{}; old bucket export-{}.",
                    canary(RUN, "new"),
                    canary(RUN, "old")
                ),
                "current_answer_omits_old",
            ),
            (
                1,
                "Current schedule unavailable.".into(),
                "current_answer_has_new",
            ),
            (
                3,
                format!("Corrected bucket export-{} only.", canary(RUN, "new")),
                "history_answer_has_old",
            ),
            (
                3,
                format!("Original bucket export-{} only.", canary(RUN, "old")),
                "history_answer_has_new",
            ),
        ] {
            let mut transcript = raw(&[&corrected()], &[&original()]);
            transcript["messages"][entry]["message"]["content"][0]["text"] = answer.into();
            let evidence = capture(&transcript);
            assert!(strict::failures(&CURRENT_AND_HISTORY, &evidence).is_empty());
            assert_gate_failed(&evidence, gate);
        }
    }
}
