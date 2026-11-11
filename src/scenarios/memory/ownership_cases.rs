//! Textual ownership relevance, not typed identity or extraction provenance.
//! Shared role, organization and place do not make both facts needed on a turn.

use super::cases::{Case, Check, Criterion, Seed};

pub const USER_OTHER_PERSON: Case = Case {
    id: "memory_user_other_person",
    title: "The user's trip versus another person's trip",
    summary: "Select the requested person's booking despite shared role, organization and city.",
    protects: "Recall tells the user's fact from another person's on the same topic. If it fails, the user's booking is confused with another person's booking.",
    baseline: false,
    distractors: false,
    memories: &[
        Seed {
            text: "The user is a project coordinator at Northstar in Lisbon; the user's travel booking reference is {own_booking}.",
            group: "own_booking",
            needed_on: &[0],
            related_on: &[],
            entities: &[],
        },
        Seed {
            text: "Maya Chen is a project coordinator at Northstar in Lisbon; Maya Chen's travel booking reference is {maya_booking}.",
            group: "maya_booking",
            needed_on: &[1],
            related_on: &[],
            entities: &[],
        },
    ],
    rules: &[],
    prompt: "What is my travel booking reference for the Northstar project coordinator trip to Lisbon?",
    followups: &["What is Maya Chen's travel booking reference for the Northstar project coordinator trip to Lisbon?"],
    // Prior valid context stays visible; it need not be forgotten or reinjected.
    compact_before: None,
    preview_first: &[],
    teacher: &[],
    teacher_reply: None,
    changes: &[],
    criteria: &[
        Criterion {
            id: "strict_delivery",
            weight: 20,
            gate: true,
            description: "Only valid recorded delivery serves each exchange's required person.",
            label: "Recorded delivery serves each exchange",
            check: Check::StrictDelivery,
        },
        Criterion {
            id: "needed_groups",
            weight: 20,
            gate: true,
            description: "Exchange zero needs the user's booking; exchange one needs Maya Chen's.",
            label: "Each exchange gets its person's booking",
            check: Check::NeededInjected { exchange: None },
        },
        Criterion {
            id: "no_extras",
            weight: 15,
            gate: true,
            description: "Shared trip vocabulary does not authorize newly delivering the other person's fact.",
            label: "Other person's booking is not delivered",
            check: Check::NothingIrrelevant,
        },
        Criterion {
            id: "no_duplicates",
            weight: 5,
            gate: true,
            description: "Do not redeliver a group while its earlier block remains visible.",
            label: "No visible fact delivered twice",
            check: Check::NoDuplicates,
        },
        Criterion {
            id: "own_answer",
            weight: 10,
            gate: true,
            description: "The first answer contains the user's run-scoped booking reference.",
            label: "First answer has the user's reference",
            check: Check::AnswerContains { exchange: Some(0), canary: "own_booking" },
        },
        Criterion {
            id: "own_answer_not_maya",
            weight: 10,
            gate: true,
            description: "The first answer does not substitute or add Maya Chen's reference.",
            label: "First answer omits Maya's reference",
            check: Check::AnswerOmits { exchange: Some(0), canary: "maya_booking" },
        },
        Criterion {
            id: "maya_answer",
            weight: 10,
            gate: true,
            description: "The follow-up answer contains Maya Chen's run-scoped booking reference.",
            label: "Follow-up answer has Maya's reference",
            check: Check::AnswerContains { exchange: Some(1), canary: "maya_booking" },
        },
        Criterion {
            id: "maya_answer_not_own",
            weight: 10,
            gate: true,
            description: "The follow-up answer does not reuse the user's still-visible reference.",
            label: "Follow-up answer omits the user's reference",
            check: Check::AnswerOmits { exchange: Some(1), canary: "own_booking" },
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{gate_completion, CompletionState, CriterionReport};
    use crate::scenarios::memory::cases::{bank, canary, render};
    use crate::scenarios::memory::score::{self, Evidence, PromptRecord, Snapshot};
    use crate::scenarios::memory::strict::{self, Failure, FailureKind};
    use serde_json::{json, Value};

    const RUN: &str = "ownership-offline-control";
    const OWN: &str = "The user is a project coordinator at Northstar in Lisbon; the user's travel booking reference is {own_booking}.";
    const MAYA: &str = "Maya Chen is a project coordinator at Northstar in Lisbon; Maya Chen's travel booking reference is {maya_booking}.";
    const GATES: [&str; 4] = [
        "strict_delivery",
        "needed_groups",
        "no_extras",
        "no_duplicates",
    ];

    // Independently specified transcript inputs: no preview, product recall,
    // scorer audit, private helper or preconstructed passing Evidence is used.
    fn raw(delivery: [&[&str]; 2], answers: [&str; 2]) -> Value {
        let mut messages = Vec::new();
        for (index, turn) in ["own_turn", "maya_turn"].iter().enumerate() {
            if !delivery[index].is_empty() {
                let lines = delivery[index]
                    .iter()
                    .map(|text| format!("- {} (2026-10-23)\n", render(text, RUN)))
                    .collect::<String>();
                messages.push(json!({
                    "entry_id": format!("e_{turn}_0_notice_0"),
                    "custom": {"custom_type": "model_notice", "data": {
                        "kind": "hook",
                        "text": format!("<memory bank=\"{}\">\nRelevant remembered memories:\n{lines}</memory>", bank(RUN))
                    }}
                }));
            }
            messages.push(json!({
                "entry_id": format!("e_{turn}_0_assistant"),
                "message": {"role": "assistant", "content": [{"type": "text", "text": answers[index]}]},
                "origin": {"turn_id": turn, "memory_bank": bank(RUN), "memory_recalled": delivery[index].len()}
            }));
        }
        json!({"messages": messages})
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            prompts: ["own_turn", "maya_turn"]
                .iter()
                .map(|turn| PromptRecord {
                    turn_id: (*turn).into(),
                    step: 0,
                    system_prompt: format!(
                        "Helpful.\n\n# Memory — bank: {}\nAutomatic memory.",
                        bank(RUN)
                    ),
                })
                .collect(),
            stored: vec![Some(render(OWN, RUN)), Some(render(MAYA, RUN))],
            ..Snapshot::default()
        }
    }

    fn evidence(raw: &Value) -> Evidence {
        score::evidence(&USER_OTHER_PERSON, RUN, raw, snapshot())
    }

    fn award(evidence: &Evidence, id: &str) -> u8 {
        score::evaluate(&USER_OTHER_PERSON, evidence)
            .awards
            .iter()
            .find(|award| award.id == id)
            .expect("declared criterion")
            .awarded
            .expect("deterministic award")
    }

    fn completed(evidence: &Evidence) -> CompletionState {
        let evaluation = score::evaluate(&USER_OTHER_PERSON, evidence);
        let reports: Vec<_> = score::criteria(&USER_OTHER_PERSON)
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

    fn fails(evidence: &Evidence, gates: &[&str], failures: &[(FailureKind, usize)]) {
        let observed = strict::failures(&USER_OTHER_PERSON, evidence);
        for &(kind, exchange) in failures {
            assert!(
                observed.contains(&Failure {
                    kind,
                    exchange: Some(exchange)
                }),
                "{observed:?}"
            );
        }
        for id in gates {
            assert_eq!(award(evidence, id), 0, "gate {id}");
        }
        assert_eq!(
            score::evaluate(&USER_OTHER_PERSON, evidence).infrastructure_error,
            None
        );
        assert_eq!(completed(evidence), CompletionState::TaskIncomplete);
    }

    #[test]
    fn capture_controls_cover_both_people_without_typed_identity_claims() {
        let own = canary(RUN, "own_booking");
        let maya = canary(RUN, "maya_booking");
        super::super::attribute_cases::assert_capture_controls(
            &USER_OTHER_PERSON,
            RUN,
            &raw([&[OWN], &[MAYA]], [&own, &maya]),
            snapshot,
        );
    }

    #[test]
    fn explicit_groups_exchanges_and_answer_contract() {
        assert_eq!(USER_OTHER_PERSON.id, "memory_user_other_person");
        assert_eq!(USER_OTHER_PERSON.memories.len(), 2);
        assert_eq!(USER_OTHER_PERSON.memories[0].text, OWN);
        assert_eq!(USER_OTHER_PERSON.memories[1].text, MAYA);
        assert_eq!(
            USER_OTHER_PERSON
                .memories
                .iter()
                .map(|s| (s.group, s.needed_on, s.related_on))
                .collect::<Vec<_>>(),
            vec![
                ("own_booking", &[0][..], &[][..]),
                ("maya_booking", &[1][..], &[][..])
            ]
        );
        assert!(USER_OTHER_PERSON
            .memories
            .iter()
            .all(|s| s.entities.is_empty()));
        assert_eq!(USER_OTHER_PERSON.prompt, "What is my travel booking reference for the Northstar project coordinator trip to Lisbon?");
        assert_eq!(USER_OTHER_PERSON.followups, &["What is Maya Chen's travel booking reference for the Northstar project coordinator trip to Lisbon?"]);
        let expected = [
            ("strict_delivery", Check::StrictDelivery),
            ("needed_groups", Check::NeededInjected { exchange: None }),
            ("no_extras", Check::NothingIrrelevant),
            ("no_duplicates", Check::NoDuplicates),
            (
                "own_answer",
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "own_booking",
                },
            ),
            (
                "own_answer_not_maya",
                Check::AnswerOmits {
                    exchange: Some(0),
                    canary: "maya_booking",
                },
            ),
            (
                "maya_answer",
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: "maya_booking",
                },
            ),
            (
                "maya_answer_not_own",
                Check::AnswerOmits {
                    exchange: Some(1),
                    canary: "own_booking",
                },
            ),
        ];
        assert_eq!(
            USER_OTHER_PERSON
                .criteria
                .iter()
                .map(|c| (c.id, c.check))
                .collect::<Vec<_>>(),
            expected
        );
        assert!(USER_OTHER_PERSON.criteria.iter().all(|c| c.gate));
        assert_eq!(
            USER_OTHER_PERSON
                .criteria
                .iter()
                .map(|c| u16::from(c.weight))
                .sum::<u16>(),
            100
        );
        let case = &USER_OTHER_PERSON;
        assert_eq!((case.baseline, case.distractors), (false, false));
        assert_eq!(USER_OTHER_PERSON.compact_before, None);
        assert!(USER_OTHER_PERSON.rules.is_empty() && USER_OTHER_PERSON.teacher.is_empty());
        assert_eq!(USER_OTHER_PERSON.teacher_reply, None);
        assert!(USER_OTHER_PERSON.changes.is_empty() && USER_OTHER_PERSON.preview_first.is_empty());
    }

    #[test]
    fn canaries_are_distinct_run_scoped_and_absent_from_questions() {
        let own = canary(RUN, "own_booking");
        let maya = canary(RUN, "maya_booking");
        assert_ne!(own, maya);
        assert_ne!(own, canary("ownership-another-run", "own_booking"));
        assert_ne!(maya, canary("ownership-another-run", "maya_booking"));
        assert!(render(OWN, RUN).contains(&own) && !render(OWN, RUN).contains(&maya));
        assert!(render(MAYA, RUN).contains(&maya) && !render(MAYA, RUN).contains(&own));
        for prompt in std::iter::once(USER_OTHER_PERSON.prompt)
            .chain(USER_OTHER_PERSON.followups.iter().copied())
        {
            assert!(!prompt.contains('{') && !prompt.contains('}'));
            assert!(!prompt.contains(&own) && !prompt.contains(&maya));
            assert!(!prompt.contains("memory::") && !prompt.contains("tool"));
        }
    }

    #[test]
    fn positive_followup_retains_prior_context_without_reinjecting_it() {
        let own = canary(RUN, "own_booking");
        let maya = canary(RUN, "maya_booking");
        let evidence = evidence(&raw([&[OWN], &[MAYA]], [&own, &maya]));
        assert_eq!(strict::failures(&USER_OTHER_PERSON, &evidence), vec![]);
        assert_eq!(
            score::evaluate(&USER_OTHER_PERSON, &evidence).infrastructure_error,
            None
        );
        assert_eq!(completed(&evidence), CompletionState::Completed);
        assert_eq!(evidence.observed.blocks[0].lines, vec![render(OWN, RUN)]);
        assert_eq!(evidence.observed.blocks[1].lines, vec![render(MAYA, RUN)]);
        assert_eq!(evidence.observed.exchanges[0].answer, own);
        assert_eq!(evidence.observed.exchanges[1].answer, maya);
        let audit = score::audit(&evidence);
        assert!(
            audit.missing.is_empty() && audit.irrelevant.is_empty() && audit.duplicates.is_empty()
        );
        let visible = crate::scenarios::memory::observe::visible_at(
            &evidence.observed,
            evidence.observed.exchanges[1].position,
        );
        assert!(visible.iter().any(|b| b.lines.contains(&render(OWN, RUN))));
    }

    #[test]
    fn neutral_correct_delivery_separates_delivery_gates_from_answer_judgment() {
        let evidence = evidence(&raw([&[OWN], &[MAYA]], ["Recorded.", "Recorded."]));
        assert!(strict::failures(&USER_OTHER_PERSON, &evidence).is_empty());
        for id in GATES {
            assert!(award(&evidence, id) > 0, "{id}");
        }
        fails(&evidence, &["own_answer", "maya_answer"], &[]);
    }

    #[test]
    fn wrong_person_despite_identical_topic_fails_raw_delivery() {
        let evidence = evidence(&raw([&[MAYA], &[OWN]], ["Recorded.", "Recorded."]));
        fails(
            &evidence,
            &["strict_delivery", "needed_groups", "no_extras"],
            &[
                (FailureKind::MissingGroup, 0),
                (FailureKind::ExtraGroup, 0),
                (FailureKind::ExtraGroup, 1),
            ],
        );
        assert!(score::audit(&evidence)
            .missing
            .contains(&(0, "own_booking".into())));
    }

    #[test]
    fn each_missing_person_and_always_omit_fail_raw_delivery() {
        for (delivery, missing) in [
            ([&[][..], &[MAYA][..]], (0, "own_booking")),
            ([&[OWN][..], &[][..]], (1, "maya_booking")),
        ] {
            let evidence = evidence(&raw(delivery, ["Recorded.", "Recorded."]));
            fails(
                &evidence,
                &["strict_delivery", "needed_groups"],
                &[(FailureKind::MissingGroup, missing.0)],
            );
            assert_eq!(
                score::audit(&evidence).missing,
                vec![(missing.0, missing.1.into())]
            );
            assert!(award(&evidence, "no_extras") > 0);
        }
        let evidence = evidence(&raw([&[], &[]], ["Recorded.", "Recorded."]));
        fails(
            &evidence,
            &["strict_delivery", "needed_groups"],
            &[
                (FailureKind::MissingGroup, 0),
                (FailureKind::MissingGroup, 1),
            ],
        );
        assert_eq!(
            score::audit(&evidence).missing,
            vec![(0, "own_booking".into()), (1, "maya_booking".into())]
        );
    }

    #[test]
    fn deliver_both_is_not_authorized_by_shared_vocabulary() {
        // Correct answers cannot rescue an objectively overbroad delivery.
        let own = canary(RUN, "own_booking");
        let maya = canary(RUN, "maya_booking");
        for answers in [["Recorded.", "Recorded."], [own.as_str(), maya.as_str()]] {
            let evidence = evidence(&raw([&[OWN, MAYA], &[OWN, MAYA]], answers));
            fails(
                &evidence,
                &["strict_delivery", "no_extras", "no_duplicates"],
                &[
                    (FailureKind::ExtraGroup, 0),
                    (FailureKind::ExtraGroup, 1),
                    (FailureKind::DuplicateGroup, 1),
                ],
            );
            assert!(award(&evidence, "needed_groups") > 0);
        }
        // A later need can use visible old context, but early delivery was
        // still unauthorized; no invented forgetting requirement is imposed.
        let evidence = evidence(&raw([&[OWN, MAYA], &[]], ["Recorded.", "Recorded."]));
        fails(
            &evidence,
            &["strict_delivery", "no_extras"],
            &[(FailureKind::ExtraGroup, 0)],
        );
        assert!(award(&evidence, "needed_groups") > 0);
    }

    #[test]
    fn duplicate_needed_fact_fails_even_when_every_needed_group_arrives() {
        for delivery in [
            [&[OWN, OWN][..], &[MAYA][..]],
            [&[OWN][..], &[MAYA, MAYA][..]],
        ] {
            let evidence = evidence(&raw(delivery, ["Recorded.", "Recorded."]));
            fails(&evidence, &["strict_delivery", "no_duplicates"], &[]);
            assert!(strict::failures(&USER_OTHER_PERSON, &evidence)
                .iter()
                .any(|f| f.kind == FailureKind::DuplicateGroup));
            assert!(award(&evidence, "needed_groups") > 0 && award(&evidence, "no_extras") > 0);
        }
    }

    #[test]
    fn answer_canaries_cannot_be_swapped_or_blended_with_valid_delivery() {
        let own = canary(RUN, "own_booking");
        let maya = canary(RUN, "maya_booking");
        let blended = format!("{own} {maya}");
        for (answers, gates) in [
            (
                [maya.as_str(), own.as_str()],
                vec![
                    "own_answer",
                    "own_answer_not_maya",
                    "maya_answer",
                    "maya_answer_not_own",
                ],
            ),
            (
                [blended.as_str(), blended.as_str()],
                vec!["own_answer_not_maya", "maya_answer_not_own"],
            ),
        ] {
            let evidence = evidence(&raw([&[OWN], &[MAYA]], answers));
            fails(&evidence, &gates, &[]);
            for id in GATES {
                assert!(award(&evidence, id) > 0, "{id}");
            }
        }
    }
}
