//! Attribute-level abstention: a shared topic is not evidence for a missing field.
//! The first exchange withholds the known fact and asks for a fixed reply
//! when the answer is unknown; the second needs that same fact.

use super::cases::{Case, Check, Criterion, Seed};

const fn gate(id: &'static str, weight: u8, check: Check, label: &'static str) -> Criterion {
    Criterion {
        id,
        weight,
        gate: true,
        description: label,
        label,
        check,
    }
}

const fn criteria(canary: &'static str) -> [Criterion; 6] {
    [
        gate(
            "strict_delivery",
            20,
            Check::StrictDelivery,
            "Recorded delivery obeys the exchange contract",
        ),
        gate(
            "known_fact_delivered",
            20,
            Check::NeededInjected { exchange: Some(1) },
            "The known attribute reaches the positive exchange",
        ),
        gate(
            "nothing_irrelevant",
            15,
            Check::NothingIrrelevant,
            "No fact delivered for sharing a topic",
        ),
        gate(
            "no_duplicates",
            15,
            Check::NoDuplicates,
            "No visible fact is delivered twice",
        ),
        gate(
            "unknown_attribute_answer",
            15,
            Check::AnswerAbstains {
                exchange: 0,
                canary,
            },
            "Unknown attribute gets the fixed abstention",
        ),
        gate(
            "known_attribute_answer",
            15,
            Check::AnswerContains {
                exchange: Some(1),
                canary,
            },
            "The known attribute answer uses its delivered value",
        ),
    ]
}

const fn known_fact(text: &'static str, group: &'static str) -> Seed {
    Seed {
        text,
        group,
        needed_on: &[1],
        related_on: &[],
        entities: &[],
    }
}

const fn attribute_case(
    id: &'static str,
    title: &'static str,
    protects: &'static str,
    prompt: &'static str,
    followups: &'static [&'static str],
    memories: &'static [Seed],
    criteria: &'static [Criterion],
) -> Case {
    Case {
        id,
        title,
        summary: "An unknown attribute must not recall a same-topic fact; the next exchange needs the known attribute.",
        protects,
        baseline: false,
        distractors: false,
        memories,
        rules: &[],
        prompt,
        followups,
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria,
    }
}

pub const KAFKA_ATTRIBUTE: Case = attribute_case(
    "memory_kafka_attribute",
    "Kafka event name does not establish partition count",
    "Memory delivers a stored attribute only when it answers the question. If it fails, a Kafka event name is offered as a partition count, or withheld when it is asked for.",
    "How many partitions does our billing Kafka topic have? If you do not know, reply exactly: I don't know",
    &["What is the exact event name published to our billing Kafka topic?"],
    &[known_fact(
        "Our billing Kafka topic publishes the event invoice-{kafka_event}.",
        "kafka_event",
    )],
    &criteria("kafka_event"),
);

pub const TRIP_ATTRIBUTE: Case = attribute_case(
    "memory_trip_attribute",
    "Trip dates do not establish the hotel",
    "Memory delivers a stored attribute only when it answers the question. If it fails, trip dates are offered as the hotel, or withheld when they are asked for.",
    "Which hotel am I staying at for my Porto trip? If you do not know, reply exactly: I don't know",
    &["What are the dates and itinerary reference for my Porto trip?"],
    &[known_fact(
        "My Porto trip runs from June 12 to June 16; its itinerary reference is trip-{trip_dates}.",
        "trip_dates",
    )],
    &criteria("trip_dates"),
);

pub const TEAM_ATTRIBUTE: Case = attribute_case(
    "memory_team_attribute",
    "Team role does not establish headcount",
    "Memory delivers a stored attribute only when it answers the question. If it fails, a role is offered as the team size, or withheld when it is asked for.",
    "How many people are on my Atlas team? If you do not know, reply exactly: I don't know",
    &["What is my role on the Atlas team, including its internal title?"],
    &[known_fact(
        "My role on the Atlas team is release coordinator, with internal title coordinator-{team_role}.",
        "team_role",
    )],
    &criteria("team_role"),
);

pub const WORKSHOP_ATTRIBUTE: Case = attribute_case(
    "memory_workshop_attribute",
    "Workshop schedule does not establish the room",
    "Memory delivers a stored attribute only when it answers the question. If it fails, a workshop schedule is offered as the room, or withheld when it is asked for.",
    "Which room is my facilitation workshop in? If you do not know, reply exactly: I don't know",
    &["What are the duration, scheduled times and session reference for my facilitation workshop?"],
    &[known_fact(
        "My facilitation workshop lasts 90 minutes, from 14:00 to 15:30; its session reference is workshop-{workshop_schedule}.",
        "workshop_schedule",
    )],
    &criteria("workshop_schedule"),
);

// Shared by the strict case modules' tests. Mutations start from each
// card's independently authored passing transcript, never from scorer output.
#[cfg(test)]
pub(super) fn assert_capture_controls(
    case: &Case,
    run: &str,
    raw: &serde_json::Value,
    snapshot: impl Fn() -> super::score::Snapshot,
) {
    use super::{score, strict};
    use crate::report::{gate_completion, CompletionState, CriterionReport};

    fn completion(
        case: &Case,
        evaluation: &crate::scenarios::ObjectiveEvaluation,
    ) -> CompletionState {
        let reports: Vec<_> = score::criteria(case)
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
    use serde_json::json;

    let first = raw["messages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|entry| entry["message"]["role"] == "assistant")
        .unwrap();
    let notice = raw["messages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|entry| entry["custom"]["data"]["kind"] == "hook")
        .unwrap();
    let evidence = score::evidence(case, run, raw, snapshot());
    let positive = score::evaluate(case, &evidence);
    assert!(strict::failures(case, &evidence).is_empty());
    assert!(positive.infrastructure_error.is_none());
    assert_eq!(completion(case, &positive), CompletionState::Completed);
    for criterion in case.criteria {
        assert_eq!(
            positive
                .awards
                .iter()
                .find(|a| a.id == criterion.id)
                .unwrap()
                .awarded,
            Some(criterion.weight)
        );
    }

    let mut mutations = Vec::new();
    let mut no_hook = raw.clone();
    no_hook["messages"][first]["origin"]
        .as_object_mut()
        .unwrap()
        .remove("memory_bank");
    mutations.push((no_hook, strict::FailureKind::HookMissing));
    let mut wrong_bank = raw.clone();
    wrong_bank["messages"][first]["origin"]["memory_bank"] = "e2e-foreign-control".into();
    mutations.push((wrong_bank, strict::FailureKind::WrongBank));
    let mut foreign_block = raw.clone();
    let body = foreign_block["messages"][notice]["custom"]["data"]["text"]
        .as_str()
        .unwrap();
    foreign_block["messages"][notice]["custom"]["data"]["text"] = body
        .replacen(
            &format!("bank=\"{}\"", super::cases::bank(run)),
            "bank=\"e2e-foreign-control\"",
            1,
        )
        .into();
    mutations.push((foreign_block, strict::FailureKind::WrongBank));
    let mut malformed = raw.clone();
    let body = malformed["messages"][notice]["custom"]["data"]["text"]
        .as_str()
        .unwrap();
    malformed["messages"][notice]["custom"]["data"]["text"] = body.replace("</memory>", "").into();
    mutations.push((malformed, strict::FailureKind::MalformedBlock));
    for count in [json!("many"), json!(99)] {
        let mut wrong_count = raw.clone();
        wrong_count["messages"][first]["origin"]["memory_recalled"] = count.clone();
        mutations.push((
            wrong_count,
            if count.is_string() {
                strict::FailureKind::InvalidAnnotation
            } else {
                strict::FailureKind::RecallCount
            },
        ));
    }
    let mut orphan = raw.clone();
    orphan["messages"][notice]["entry_id"] = "e_orphan_0_notice_0".into();
    mutations.push((orphan, strict::FailureKind::OrphanBlock));
    let mut missing_exchange = raw.clone();
    missing_exchange["messages"]
        .as_array_mut()
        .unwrap()
        .remove(first);
    mutations.push((missing_exchange, strict::FailureKind::ExchangeCount));
    let mut empty_answer = raw.clone();
    empty_answer["messages"][first]["message"]["content"] = json!([]);
    mutations.push((empty_answer, strict::FailureKind::EmptyAnswer));
    mutations.push((json!({"messages": []}), strict::FailureKind::ExchangeCount));

    for (mutated, expected) in mutations {
        // Repeat with the independent observer missing: observed defects must
        // still be objective failures, not an infrastructure retry or a pass.
        for missing_observer in [false, true] {
            let mut recorded = snapshot();
            if missing_observer {
                recorded.prompts.clear();
            }
            let evidence = score::evidence(case, run, &mutated, recorded);
            assert!(
                strict::failures(case, &evidence)
                    .iter()
                    .any(|f| f.kind == expected),
                "{}: {expected:?}",
                case.id
            );
            let evaluation = score::evaluate(case, &evidence);
            let gate = evaluation
                .awards
                .iter()
                .find(|a| a.id == "strict_delivery")
                .unwrap();
            assert_eq!(gate.awarded, Some(0), "{}: {expected:?}", case.id);
            assert!(gate
                .reason
                .contains(&serde_json::to_string(&expected).unwrap()));
            assert_eq!(
                completion(case, &evaluation),
                CompletionState::TaskIncomplete
            );
            assert!(evaluation.infrastructure_error.is_none());
        }
    }
    // Missing observer/transcript alone is infrastructure, never evidence of
    // a worker defect. An actually recorded empty transcript above is different.
    for (transcript, expected) in [
        (raw.clone(), strict::FailureKind::PromptUnavailable),
        (json!({}), strict::FailureKind::TranscriptUnavailable),
    ] {
        let evidence = score::evidence(case, run, &transcript, score::Snapshot::default());
        assert!(strict::failures(case, &evidence)
            .iter()
            .any(|f| f.kind == expected));
        assert!(!strict::has_delivery_failure(case, &evidence));
        let evaluation = score::evaluate(case, &evidence);
        assert!(evaluation.infrastructure_error.is_some());
        assert_eq!(
            completion(case, &evaluation),
            CompletionState::TaskIncomplete
        );
        assert_eq!(
            evaluation
                .awards
                .iter()
                .find(|a| a.id == "strict_delivery")
                .unwrap()
                .awarded,
            Some(0)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{gate_completion, CompletionState, CriterionReport};
    use crate::scenarios::memory::{
        cases,
        score::{self, Evidence, PromptRecord, Snapshot},
        strict::{self, FailureKind},
    };
    use serde_json::{json, Value};

    // Independent contract data, not derived from preview, audit, ranking or
    // the case's permission lists. These are constructor/evaluator controls,
    // not evidence that a live memory worker retrieved or abstained correctly.
    struct Contract {
        case: &'static Case,
        id: &'static str,
        group: &'static str,
        seed: &'static str,
        negative: &'static str,
        positive: &'static str,
        absent_field: &'static str,
    }

    const CONTRACTS: &[Contract] = &[
        Contract {
            case: &KAFKA_ATTRIBUTE,
            id: "memory_kafka_attribute",
            group: "kafka_event",
            seed: "Our billing Kafka topic publishes the event invoice-{kafka_event}.",
            negative: "How many partitions does our billing Kafka topic have? If you do not know, reply exactly: I don't know",
            positive: "What is the exact event name published to our billing Kafka topic?",
            absent_field: "partition",
        },
        Contract {
            case: &TRIP_ATTRIBUTE,
            id: "memory_trip_attribute",
            group: "trip_dates",
            seed: "My Porto trip runs from June 12 to June 16; its itinerary reference is trip-{trip_dates}.",
            negative: "Which hotel am I staying at for my Porto trip? If you do not know, reply exactly: I don't know",
            positive: "What are the dates and itinerary reference for my Porto trip?",
            absent_field: "hotel",
        },
        Contract {
            case: &TEAM_ATTRIBUTE,
            id: "memory_team_attribute",
            group: "team_role",
            seed: "My role on the Atlas team is release coordinator, with internal title coordinator-{team_role}.",
            negative: "How many people are on my Atlas team? If you do not know, reply exactly: I don't know",
            positive: "What is my role on the Atlas team, including its internal title?",
            absent_field: "headcount",
        },
        Contract {
            case: &WORKSHOP_ATTRIBUTE,
            id: "memory_workshop_attribute",
            group: "workshop_schedule",
            seed: "My facilitation workshop lasts 90 minutes, from 14:00 to 15:30; its session reference is workshop-{workshop_schedule}.",
            negative: "Which room is my facilitation workshop in? If you do not know, reply exactly: I don't know",
            positive: "What are the duration, scheduled times and session reference for my facilitation workshop?",
            absent_field: "room",
        },
    ];

    const RUN: &str = "attribute-control";
    const BANK: &str = "e2e-attribute-control";
    const SYSTEM: &str = "Helpful.\n\n# Memory — bank: e2e-attribute-control\nAutomatic memory.";

    fn stored(contract: &Contract) -> String {
        // Only template substitution uses the run-canary API; expected text
        // and required/forbidden exchanges come from the independent table.
        contract.seed.replace(
            &format!("{{{}}}", contract.group),
            &cases::canary(RUN, contract.group),
        )
    }

    fn raw_fixture(
        contract: &Contract,
        negative_lines: &[&str],
        positive_lines: &[&str],
        answers: [&str; 2],
    ) -> (Value, Snapshot) {
        let mut messages = Vec::<Value>::new();
        for (index, (turn, lines, prompt)) in [
            ("t_negative", negative_lines, contract.negative),
            ("t_positive", positive_lines, contract.positive),
        ]
        .into_iter()
        .enumerate()
        {
            messages.push(json!({"message": {"role": "user", "content": prompt}}));
            if !lines.is_empty() {
                let body = lines
                    .iter()
                    .map(|line| format!("- {line} (2026-10-23)\n"))
                    .collect::<String>();
                messages.push(json!({
                    "entry_id": format!("e_{turn}_0_notice_0"),
                    "custom": {"custom_type": "model_notice", "data": {
                        "kind": "hook",
                        "text": format!("<memory bank=\"{BANK}\">\nRelevant remembered memories:\n{body}</memory>")
                    }}
                }));
            }
            messages.push(json!({
                "entry_id": format!("e_{turn}_0_assistant"),
                "message": {"role": "assistant", "content": [{"type": "text", "text": answers[index]}]},
                "origin": {"turn_id": turn, "memory_bank": BANK, "memory_recalled": lines.len(),
                    "req": {"system_sha": score::system_sha(SYSTEM)}}
            }));
        }
        let snapshot = Snapshot {
            prompts: ["t_negative", "t_positive"]
                .into_iter()
                .map(|turn| PromptRecord {
                    turn_id: turn.into(),
                    step: 0,
                    system_prompt: SYSTEM.into(),
                })
                .collect(),
            stored: vec![Some(stored(contract))],
            ..Snapshot::default()
        };
        (json!({"messages": messages}), snapshot)
    }

    fn fixture(
        contract: &Contract,
        negative: &[&str],
        positive: &[&str],
        answers: [&str; 2],
    ) -> Evidence {
        let (raw, snapshot) = raw_fixture(contract, negative, positive, answers);
        score::evidence(contract.case, RUN, &raw, snapshot)
    }

    #[test]
    fn capture_defects_and_missing_observer_are_distinct_for_every_attribute() {
        for contract in CONTRACTS {
            let fact = stored(contract);
            let make = || raw_fixture(contract, &[], &[&fact], ["I don't know.", &fact]);
            let (raw, _) = make();
            assert_capture_controls(contract.case, RUN, &raw, || make().1);
        }
    }

    fn award(case: &Case, evidence: &Evidence, id: &str) -> u32 {
        let evaluation = score::evaluate(case, evidence);
        let awarded = evaluation
            .awards
            .iter()
            .find(|award| award.id == id)
            .expect("the contract criterion exists")
            .awarded
            .expect("a mechanical criterion has an award");
        u32::from(awarded)
    }

    fn completion(case: &Case, evidence: &Evidence) -> CompletionState {
        let evaluation = score::evaluate(case, evidence);
        let reports: Vec<_> = score::criteria(case)
            .iter()
            .map(|criterion| {
                let award = evaluation
                    .awards
                    .iter()
                    .find(|award| award.id == criterion.id)
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

    fn has_failure(evidence: &Evidence, case: &Case, kind: FailureKind, exchange: usize) -> bool {
        strict::failures(case, evidence)
            .iter()
            .any(|failure| failure.kind == kind && failure.exchange == Some(exchange))
    }

    #[test]
    fn contracts_require_only_the_supported_second_exchange() {
        assert_eq!(CONTRACTS.len(), 4);
        let mut ids = std::collections::BTreeSet::new();
        for contract in CONTRACTS {
            let case = contract.case;
            assert_eq!(case.id, contract.id);
            assert!(ids.insert(case.id));
            assert_eq!(case.prompt, contract.negative);
            assert_eq!(case.followups, &[contract.positive]);
            assert!(!case.baseline && !case.distractors);
            assert!(case.rules.is_empty() && case.changes.is_empty());
            assert!(case.teacher.is_empty() && case.teacher_reply.is_none());
            assert!(case.compact_before.is_none() && case.preview_first.is_empty());
            assert_eq!(case.memories.len(), 1);
            let seed = &case.memories[0];
            assert_eq!(seed.text, contract.seed);
            assert_eq!(seed.group, contract.group);
            assert_eq!(seed.needed_on, &[1]);
            assert!(seed.related_on.is_empty() && seed.entities.is_empty());
            let expected_groups: [&[&str]; 2] = [&[], &[contract.group]];
            for (exchange, expected) in expected_groups.into_iter().enumerate() {
                let required: Vec<_> = case
                    .memories
                    .iter()
                    .filter(|seed| seed.needed_on.contains(&exchange))
                    .map(|seed| seed.group)
                    .collect();
                assert_eq!(required, expected);
            }
            let expected_checks = [
                Check::StrictDelivery,
                Check::NeededInjected { exchange: Some(1) },
                Check::NothingIrrelevant,
                Check::NoDuplicates,
                Check::AnswerAbstains {
                    exchange: 0,
                    canary: contract.group,
                },
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: contract.group,
                },
            ];
            assert_eq!(case.criteria.len(), expected_checks.len());
            for (criterion, check) in case.criteria.iter().zip(expected_checks) {
                assert!(criterion.gate && criterion.weight > 0);
                assert_eq!(criterion.check, check);
            }
            assert_eq!(
                case.criteria
                    .iter()
                    .map(|c| u32::from(c.weight))
                    .sum::<u32>(),
                100
            );
        }
    }

    #[test]
    fn queries_do_not_leak_canaries_or_turn_into_tool_instructions() {
        for contract in CONTRACTS {
            for prompt in [contract.negative, contract.positive] {
                assert!(!prompt.contains(['{', '}']));
                for forbidden in [contract.group, "memory::", "inject", "preview", "canary"] {
                    assert!(!prompt.contains(forbidden), "{}: {forbidden}", contract.id);
                }
                for run in [RUN, "attribute-another-attempt"] {
                    assert!(!prompt.contains(&cases::canary(run, contract.group)));
                }
            }
        }
        assert_ne!(
            cases::canary(RUN, "trip_dates"),
            cases::canary("attribute-another-attempt", "trip_dates")
        );
    }

    #[test]
    fn other_fields_numbers_and_dates_do_not_supply_the_missing_attribute() {
        for contract in CONTRACTS {
            assert!(!contract.seed.to_lowercase().contains(contract.absent_field));
        }
        // Dates are not a hotel; duration and times are not a room number.
        assert!(CONTRACTS[1].seed.contains("June 12 to June 16"));
        assert!(CONTRACTS[3]
            .seed
            .contains("90 minutes, from 14:00 to 15:30"));
        assert!(!CONTRACTS[0].seed.contains("partition"));
        assert!(!CONTRACTS[2].seed.contains("people"));
        // This is a local data contract, not an oracle based on product scores.
    }

    #[test]
    fn independent_negative_then_positive_delivery_passes_all_gates() {
        for contract in CONTRACTS {
            let fact = stored(contract);
            let evidence = fixture(contract, &[], &[&fact], ["I don't know.", &fact]);
            assert!(strict::failures(contract.case, &evidence).is_empty());
            let evaluation = score::evaluate(contract.case, &evidence);
            assert!(evaluation.infrastructure_error.is_none());
            for criterion in contract.case.criteria {
                assert_eq!(
                    award(contract.case, &evidence, criterion.id),
                    u32::from(criterion.weight)
                );
            }
            assert_eq!(
                completion(contract.case, &evidence),
                CompletionState::Completed
            );
        }
    }

    #[test]
    fn same_topic_delivery_on_the_unknown_attribute_is_a_hard_failure() {
        for contract in CONTRACTS {
            let fact = stored(contract);
            let evidence = fixture(contract, &[&fact], &[], ["I don't know.", &fact]);
            assert!(has_failure(
                &evidence,
                contract.case,
                FailureKind::ExtraGroup,
                0
            ));
            assert_eq!(award(contract.case, &evidence, "strict_delivery"), 0);
            assert_eq!(award(contract.case, &evidence, "nothing_irrelevant"), 0);
            assert_eq!(
                completion(contract.case, &evidence),
                CompletionState::TaskIncomplete
            );
            assert!(score::evaluate(contract.case, &evidence)
                .infrastructure_error
                .is_none());
        }
    }

    #[test]
    fn always_abstaining_cannot_pass_the_supported_attribute() {
        for contract in CONTRACTS {
            let fact = stored(contract);
            let evidence = fixture(contract, &[], &[&fact], ["I don't know.", "I don't know."]);
            assert!(strict::failures(contract.case, &evidence).is_empty());
            assert_eq!(award(contract.case, &evidence, "known_attribute_answer"), 0);
            assert_eq!(
                completion(contract.case, &evidence),
                CompletionState::TaskIncomplete
            );
            assert!(score::evaluate(contract.case, &evidence)
                .infrastructure_error
                .is_none());
            let withheld = fixture(contract, &[], &[], ["I don't know.", "I don't know."]);
            assert!(has_failure(
                &withheld,
                contract.case,
                FailureKind::MissingGroup,
                1
            ));
            assert_eq!(award(contract.case, &withheld, "known_fact_delivered"), 0);
            assert_eq!(award(contract.case, &withheld, "strict_delivery"), 0);
            assert_eq!(
                completion(contract.case, &withheld),
                CompletionState::TaskIncomplete
            );
            assert!(score::evaluate(contract.case, &withheld)
                .infrastructure_error
                .is_none());
        }
    }

    #[test]
    fn only_the_fixed_abstention_passes_the_unknown_attribute() {
        let contract = &CONTRACTS[1];
        let fact = stored(contract);
        for text in [
            "I don't know",
            "I don't know.",
            "i don\u{2019}t know.",
            "  I DON'T KNOW\n",
        ] {
            let evidence = fixture(contract, &[], &[&fact], [text, &fact]);
            assert_eq!(
                award(contract.case, &evidence, "unknown_attribute_answer"),
                15,
                "{text}"
            );
            assert_eq!(
                completion(contract.case, &evidence),
                CompletionState::Completed
            );
        }
        for text in [
            "Your hotel is the Ritz.",
            "I don't know. Your hotel is the Ritz.",
            "I don't know your hotel is Porto.",
            "I have no information about your hotel.",
            "Unknown: hotel 42.",
            "",
        ] {
            let evidence = fixture(contract, &[], &[&fact], [text, &fact]);
            assert_eq!(
                award(contract.case, &evidence, "unknown_attribute_answer"),
                0,
                "{text}"
            );
            assert_eq!(
                completion(contract.case, &evidence),
                CompletionState::TaskIncomplete
            );
        }
    }

    #[test]
    fn negative_answer_canary_leak_is_rejected_even_without_a_notice() {
        for contract in CONTRACTS {
            let fact = stored(contract);
            let evidence = fixture(contract, &[], &[&fact], [&fact, &fact]);
            assert!(strict::failures(contract.case, &evidence).is_empty());
            assert_eq!(
                award(contract.case, &evidence, "unknown_attribute_answer"),
                0
            );
            assert_eq!(
                completion(contract.case, &evidence),
                CompletionState::TaskIncomplete
            );
            assert!(score::evaluate(contract.case, &evidence)
                .infrastructure_error
                .is_none());
        }
    }

    #[test]
    fn unrelated_fields_and_duplicate_positive_delivery_are_rejected() {
        for contract in CONTRACTS {
            let fact = stored(contract);
            let unrelated = fixture(
                contract,
                &["The office has 27 desks."],
                &[&fact],
                ["I don't know.", &fact],
            );
            assert!(has_failure(
                &unrelated,
                contract.case,
                FailureKind::ExtraGroup,
                0
            ));
            assert_eq!(award(contract.case, &unrelated, "nothing_irrelevant"), 0);
            assert_eq!(award(contract.case, &unrelated, "strict_delivery"), 0);
            assert_eq!(
                completion(contract.case, &unrelated),
                CompletionState::TaskIncomplete
            );
            assert!(score::evaluate(contract.case, &unrelated)
                .infrastructure_error
                .is_none());
            let duplicate = fixture(contract, &[], &[&fact, &fact], ["I don't know.", &fact]);
            assert!(has_failure(
                &duplicate,
                contract.case,
                FailureKind::DuplicateGroup,
                1
            ));
            assert_eq!(award(contract.case, &duplicate, "no_duplicates"), 0);
            assert_eq!(award(contract.case, &duplicate, "strict_delivery"), 0);
            assert_eq!(
                completion(contract.case, &duplicate),
                CompletionState::TaskIncomplete
            );
            assert!(score::evaluate(contract.case, &duplicate)
                .infrastructure_error
                .is_none());
        }
    }
}
