//! Judges a memory session: what was injected where, against what each
//! exchange needed, and what the subject answered. Deterministic: no model.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::cases::{bank, canary, labels, render, Case, Check, Criterion, DISTRACTORS};
use super::observe::{observe, visible_at, MemoryBlock, Observed};
use crate::report::EvaluationDimension;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::{CriterionSpec, ObjectiveEvaluation};

const SCHEMA: &str = "memory-evaluation/1";
/// Where memory's section starts in the system prompt.
const SECTION_START: &str = "\n\n# Memory — bank: ";
const RULE_TRUNCATED: &str = "[rule truncated";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedEvidence {
    pub text: String,
    pub group: String,
    pub needed_on: Vec<usize>,
    /// False when the case withheld the seed (a baseline, or unused distractors).
    pub planted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleEvidence {
    pub name: String,
    pub content: String,
    pub planted: bool,
}

/// One system prompt the observer saw, as Harness was about to send it.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptRecord {
    pub turn_id: String,
    pub step: u64,
    pub system_prompt: String,
}

/// What the observer's records say about one exchange's step-0 prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExchangePrompt {
    pub recorded: bool,
    /// A recorded prompt hashes to the `system_sha` Harness stamped; `None`
    /// when it stamped none.
    pub sha_matches: Option<bool>,
    pub memory_section: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub id: String,
    pub value: f64,
    pub unit: String,
}

/// How memory ranked recalls during the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Retrieval {
    /// MiniLM vectors next to BM25 and entities.
    Minilm,
    /// BM25 and entities only: no embedder answered.
    Lexical,
}

/// The captured deliverable of a memory case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub schema: String,
    pub bank: String,
    /// `None` when setup never asked the doctor.
    pub retrieval: Option<Retrieval>,
    /// Label -> value, for every canary the case plants or checks for.
    pub canaries: BTreeMap<String, String>,
    pub seeds: Vec<SeedEvidence>,
    pub rules: Vec<RuleEvidence>,
    pub observed: Observed,
    /// Parallel to `observed.exchanges`.
    pub prompts: Vec<ExchangePrompt>,
    pub compaction_status: Option<String>,
    pub measurements: Vec<Measurement>,
}

/// Lines are `(exchange, text)`; `missing` is `(exchange, group)`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Audit {
    /// A group an exchange needs that is neither injected there nor still visible.
    pub missing: Vec<(usize, String)>,
    /// Injected lines their exchange does not need, or that no seed names.
    pub irrelevant: Vec<(usize, String)>,
    /// Injected lines whose group was already injected and is still visible.
    pub duplicates: Vec<(usize, String)>,
    pub relevant: usize,
    /// Every line judged, in order.
    pub injected: Vec<(usize, String)>,
}

/// Hex sha256 of the JSON string, as Harness hashes the system prompt into
/// `origin.req.system_sha`.
pub fn system_sha(prompt: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(prompt).expect("a string serializes"))
    )
}

/// Memory's section of a system prompt: its header to the end.
pub fn memory_section(prompt: &str) -> Option<&str> {
    prompt.find(SECTION_START).map(|start| &prompt[start..])
}

pub fn evidence(
    case: &Case,
    run_id: &str,
    transcript: &Value,
    prompts: &[PromptRecord],
    compaction_status: Option<String>,
    retrieval: Option<Retrieval>,
) -> Evidence {
    assemble(
        case,
        run_id,
        observe(transcript),
        prompts,
        compaction_status,
        retrieval,
    )
}

fn assemble(
    case: &Case,
    run_id: &str,
    observed: Observed,
    records: &[PromptRecord],
    compaction_status: Option<String>,
    retrieval: Option<Retrieval>,
) -> Evidence {
    let planted = !case.baseline;
    let distractors = DISTRACTORS.iter().map(|text| SeedEvidence {
        text: (*text).to_string(),
        group: "distractor".to_string(),
        needed_on: Vec::new(),
        planted: planted && case.distractors,
    });
    let memories = case.memories.iter().map(|seed| SeedEvidence {
        text: render(seed.text, run_id),
        group: seed.group.to_string(),
        needed_on: seed.needed_on.to_vec(),
        planted,
    });
    let prompts = observed
        .exchanges
        .iter()
        .map(|exchange| {
            let seen: Vec<&PromptRecord> = records
                .iter()
                .filter(|record| record.turn_id == exchange.turn_id && record.step == 0)
                .collect();
            let matching = seen.iter().find(|record| {
                exchange.system_sha.as_deref() == Some(system_sha(&record.system_prompt).as_str())
            });
            ExchangePrompt {
                recorded: !seen.is_empty(),
                sha_matches: exchange.system_sha.as_ref().map(|_| matching.is_some()),
                memory_section: matching
                    .or(seen.last())
                    .and_then(|record| memory_section(&record.system_prompt))
                    .map(str::to_string),
            }
        })
        .collect();
    let mut evidence = Evidence {
        schema: SCHEMA.to_string(),
        bank: bank(run_id),
        retrieval,
        canaries: labels(case)
            .into_iter()
            .map(|label| (label.to_string(), canary(run_id, label)))
            .collect(),
        seeds: distractors.chain(memories).collect(),
        rules: case
            .rules
            .iter()
            .map(|rule| RuleEvidence {
                name: rule.name.to_string(),
                content: render(rule.content, run_id),
                planted,
            })
            .collect(),
        observed,
        prompts,
        compaction_status,
        measurements: Vec::new(),
    };
    evidence.measurements = measurements(&evidence);
    evidence
}

pub fn audit(evidence: &Evidence) -> Audit {
    // Another bank's block comes with an infrastructure error, never a score.
    let own = Observed {
        blocks: evidence
            .observed
            .blocks
            .iter()
            .filter(|block| block.bank == evidence.bank)
            .cloned()
            .collect(),
        ..evidence.observed.clone()
    };
    let observed = &own;
    let seed_of = |line: &str| evidence.seeds.iter().find(|seed| seed.text == line);
    let holds = |block: &MemoryBlock, group: &str| {
        block
            .lines
            .iter()
            .any(|line| seed_of(line).is_some_and(|seed| seed.group == group))
    };
    let mut audit = Audit::default();
    for block in &observed.blocks {
        // A block of a turn that never answered has no exchange to judge it against.
        let Some(exchange) = observed
            .exchanges
            .iter()
            .position(|exchange| exchange.turn_id == block.turn_id)
        else {
            continue;
        };
        let earlier = visible_at(observed, block.position);
        for (index, line) in block.lines.iter().enumerate() {
            audit.injected.push((exchange, line.clone()));
            let Some(seed) = seed_of(line).filter(|seed| seed.needed_on.contains(&exchange)) else {
                audit.irrelevant.push((exchange, line.clone()));
                continue;
            };
            let repeated = block.lines[..index]
                .iter()
                .any(|before| seed_of(before).is_some_and(|other| other.group == seed.group))
                || earlier.iter().any(|before| holds(before, &seed.group));
            if repeated {
                audit.duplicates.push((exchange, line.clone()));
            } else {
                audit.relevant += 1;
            }
        }
    }
    let needs: BTreeSet<(usize, &str)> = evidence
        .seeds
        .iter()
        .flat_map(|seed| {
            seed.needed_on
                .iter()
                .map(move |&exchange| (exchange, seed.group.as_str()))
        })
        .collect();
    for (index, group) in needs {
        let served = observed.exchanges.get(index).is_some_and(|exchange| {
            let own = observed
                .blocks
                .iter()
                .filter(|block| block.turn_id == exchange.turn_id);
            own.chain(visible_at(observed, exchange.position))
                .any(|block| holds(block, group))
        });
        if !served {
            audit.missing.push((index, group.to_string()));
        }
    }
    audit
}

fn measurements(evidence: &Evidence) -> Vec<Measurement> {
    let audit = audit(evidence);
    let observed = &evidence.observed;
    let count = |id: &str, value: usize| Measurement {
        id: id.to_string(),
        value: value as f64,
        unit: "count".to_string(),
    };
    // Memory's own unit: bytes / 4, rounded up.
    let tokens = |id: &str, bytes: usize| Measurement {
        unit: "tokens".to_string(),
        ..count(id, bytes.div_ceil(4))
    };
    vec![
        count("memory_expected_injected", audit.relevant),
        count("memory_irrelevant_injected", audit.irrelevant.len()),
        count("memory_duplicate_injections", audit.duplicates.len()),
        tokens(
            "memory_injected_tokens",
            observed.blocks.iter().map(|block| block.memory_bytes).sum(),
        ),
        count(
            "memory_turns_without_hook",
            observed
                .exchanges
                .iter()
                .filter(|exchange| exchange.memory_bank.is_none())
                .count(),
        ),
        tokens(
            "memory_rules_tokens",
            first_section(evidence).map_or(0, str::len),
        ),
        tokens(
            "memory_reminder_tokens",
            observed
                .blocks
                .iter()
                .map(|block| block.reminder_bytes)
                .sum(),
        ),
    ]
}

fn first_section(evidence: &Evidence) -> Option<&str> {
    evidence
        .prompts
        .first()
        .and_then(|prompt| prompt.memory_section.as_deref())
}

fn spec(criterion: &Criterion) -> AssessmentSpec {
    let dimension = EvaluationDimension::Deliverable;
    let spec = if criterion.gate {
        AssessmentSpec::gate_in(
            criterion.id,
            criterion.weight,
            criterion.description,
            dimension,
        )
    } else {
        AssessmentSpec::scored_in(
            criterion.id,
            criterion.weight,
            criterion.description,
            dimension,
        )
    };
    spec.labeled(criterion.label)
}

/// The case's criteria as the runner declares them.
pub fn criteria(case: &Case) -> Vec<CriterionSpec> {
    let specs: Vec<AssessmentSpec> = case.criteria.iter().map(spec).collect();
    assessment::criteria(&specs)
}

pub fn evaluate(case: &Case, evidence: &Evidence) -> ObjectiveEvaluation {
    let audit = audit(evidence);
    let mut evaluation = assessment::build_evaluation(case.criteria.iter().map(|criterion| {
        let (satisfied, details) = judge(criterion.check, case, evidence, &audit);
        spec(criterion).full_or_zero(satisfied, details)
    }));
    evaluation.infrastructure_error = infrastructure_error(case, evidence);
    evaluation
}

fn judge(check: Check, case: &Case, evidence: &Evidence, audit: &Audit) -> (bool, String) {
    match check {
        Check::NeededInjected => verdict(
            &audit.missing,
            "every needed memory reached its exchange",
            "needed but not injected or visible",
        ),
        Check::NothingIrrelevant => verdict(
            &audit.irrelevant,
            "every injected memory was needed",
            "injected without being needed",
        ),
        Check::NoDuplicates => verdict(
            &audit.duplicates,
            "no memory was injected twice while visible",
            "injected again while its group was still visible",
        ),
        Check::GroupOmitted { group } => {
            let found: Vec<(usize, String)> = audit
                .injected
                .iter()
                .filter(|(_, line)| {
                    evidence
                        .seeds
                        .iter()
                        .any(|seed| seed.text == *line && seed.group == group)
                })
                .cloned()
                .collect();
            verdict(
                &found,
                &format!("no {group} memory was injected"),
                &format!("{group} memory injected"),
            )
        }
        Check::RulesInPrompt => {
            let section = first_section(evidence);
            let lacking: Vec<&str> = evidence
                .rules
                .iter()
                .filter(|rule| section.is_none_or(|section| !section.contains(&rule.content)))
                .map(|rule| rule.name.as_str())
                .collect();
            let truncated = section.is_some_and(|section| section.contains(RULE_TRUNCATED));
            let satisfied = section.is_some() && lacking.is_empty() && !truncated;
            let details = match (section, truncated) {
                (None, _) => "the first prompt has no memory section".to_string(),
                (_, true) => "memory truncated a rule".to_string(),
                _ if lacking.is_empty() => "every rule is in the system prompt".to_string(),
                _ => format!(
                    "rules missing from the system prompt: {}",
                    lacking.join(", ")
                ),
            };
            (satisfied, details)
        }
        Check::AnswerContains { exchange, canary } => {
            let Some(value) = evidence.canaries.get(canary) else {
                return (false, format!("no canary recorded for {canary}"));
            };
            let wanted = exchange.map_or(0..=case.followups.len(), |index| index..=index);
            let lacking: Vec<usize> = wanted
                .filter(|&index| !answer(evidence, index).contains(value.as_str()))
                .collect();
            if lacking.is_empty() {
                (true, format!("the answer carries {canary} ({value})"))
            } else {
                (
                    false,
                    format!("exchanges without {canary} ({value}): {lacking:?}"),
                )
            }
        }
        Check::AnswerOmits { canary } => {
            let Some(value) = evidence.canaries.get(canary) else {
                return (false, format!("no canary recorded for {canary}"));
            };
            // An exchange that never answered cannot show the value was left out.
            let leaking: Vec<usize> = (0..=case.followups.len())
                .filter(|&index| {
                    evidence
                        .observed
                        .exchanges
                        .get(index)
                        .is_none_or(|exchange| exchange.answer.contains(value.as_str()))
                })
                .collect();
            if leaking.is_empty() {
                (true, format!("no answer carries {canary} ({value})"))
            } else {
                (false, format!("exchanges that answered with {canary} ({value}), or never answered: {leaking:?}"))
            }
        }
    }
}

fn verdict(found: &[(usize, String)], clean: &str, problem: &str) -> (bool, String) {
    if found.is_empty() {
        return (true, clean.to_string());
    }
    let listed: Vec<String> = found
        .iter()
        .map(|(exchange, what)| format!("exchange {exchange}: {what}"))
        .collect();
    (false, format!("{problem}: {}", listed.join("; ")))
}

/// "" when the exchange is missing.
fn answer(evidence: &Evidence, index: usize) -> &str {
    evidence
        .observed
        .exchanges
        .get(index)
        .map_or("", |exchange| exchange.answer.as_str())
}

/// A run that cannot show memory ran is an infrastructure failure, not a score.
fn infrastructure_error(case: &Case, evidence: &Evidence) -> Option<String> {
    let mut problems = Vec::new();
    // A missing exchange must never read as "nothing injected": every check
    // below walks the exchanges that exist, so a short observation would pass.
    let expected = 1 + case.followups.len();
    let seen = evidence.observed.exchanges.len();
    if seen != expected {
        problems.push(format!(
            "observed {seen} exchanges, the case sends {expected}"
        ));
    }
    for (index, exchange) in evidence.observed.exchanges.iter().enumerate() {
        // A bank other than the attempt's means the turn-metadata pin never
        // reached memory: it read `main` or a directory bank instead.
        match exchange.memory_bank.as_deref() {
            None => problems.push(format!("exchange {index} ran without the memory hook")),
            Some(read) if read != evidence.bank => problems.push(format!(
                "exchange {index}: memory read bank {read}, not the attempt's {}",
                evidence.bank
            )),
            Some(_) => {}
        }
        // A block that failed to parse vanishes; memory's own count catches it.
        let parsed: usize = evidence
            .observed
            .blocks
            .iter()
            .filter(|block| block.turn_id == exchange.turn_id && block.step == 0)
            .map(|block| block.lines.len())
            .sum();
        if parsed != exchange.memory_recalled {
            problems.push(format!(
                "exchange {index}: memory recalled {} memories, {parsed} lines parsed",
                exchange.memory_recalled
            ));
        }
        // shortcut: the published Harness stamps no `origin.req`, so an unstamped step
        // trusts its recorded prompt unchecked; once a Harness release stamps every
        // step, change `!= Some(false)` to `== Some(true)`.
        match evidence.prompts.get(index) {
            Some(prompt) if prompt.recorded && prompt.sha_matches != Some(false) => {}
            Some(prompt) if prompt.recorded => problems.push(format!(
                "exchange {index}: no recorded prompt matches the system_sha Harness stamped"
            )),
            _ => problems.push(format!("exchange {index}: the observer recorded no prompt")),
        }
    }
    if case.compact_before.is_some() && evidence.compaction_status.as_deref() != Some("ok") {
        problems.push(format!(
            "the forced compaction did not report ok (status: {})",
            evidence.compaction_status.as_deref().unwrap_or("none")
        ));
    }
    (!problems.is_empty()).then(|| problems.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::memory::cases::{bank, canary, render, Case, CASES, DISTRACTORS};
    use crate::scenarios::memory::observe::{Compaction, Exchange, MemoryBlock, Observed};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    const RUN: &str = "a";
    const PROMPT: &str = "You are helpful.";
    const HEADER: &str = "\n\n# Memory — bank: e2e-a\nMemory is automatic here.\n";

    fn case_named(id: &str) -> &'static Case {
        CASES
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("no case {id}"))
    }

    /// Rendered text of the case's `index`-th memory.
    fn line(case: &Case, index: usize) -> String {
        render(case.memories[index].text, RUN)
    }

    /// Exchange `index` answers at position `10 * index + 2`; its block sits
    /// just before, at `10 * index + 1`.
    fn exchange(index: usize, answer: &str) -> Exchange {
        Exchange {
            turn_id: format!("t{index}"),
            position: 10 * index + 2,
            memory_bank: Some(bank(RUN)),
            system_sha: None,
            memory_recalled: 0,
            answer: answer.into(),
        }
    }

    fn block(index: usize, lines: Vec<String>) -> MemoryBlock {
        MemoryBlock {
            bank: bank(RUN),
            turn_id: format!("t{index}"),
            step: 0,
            position: 10 * index + 1,
            memory_bytes: lines.iter().map(String::len).sum(),
            reminder_bytes: 0,
            lines,
        }
    }

    /// Each exchange's `memory_recalled` counts its own step-0 block's lines.
    fn observed(answers: &[&str], blocks: Vec<MemoryBlock>) -> Observed {
        Observed {
            exchanges: answers
                .iter()
                .enumerate()
                .map(|(index, answer)| Exchange {
                    memory_recalled: blocks
                        .iter()
                        .filter(|block| block.turn_id == format!("t{index}") && block.step == 0)
                        .map(|block| block.lines.len())
                        .sum(),
                    ..exchange(index, answer)
                })
                .collect(),
            blocks,
            compactions: Vec::new(),
        }
    }

    /// Every exchange saw `prompt`, and its recorded sha says so.
    fn evidence_with(
        case: &Case,
        mut observed: Observed,
        prompt: &str,
        status: Option<&str>,
    ) -> Evidence {
        for exchange in &mut observed.exchanges {
            exchange.system_sha = Some(system_sha(prompt));
        }
        let records: Vec<PromptRecord> = observed
            .exchanges
            .iter()
            .map(|exchange| record(&exchange.turn_id, 0, prompt))
            .collect();
        assemble(
            case,
            RUN,
            observed,
            &records,
            status.map(str::to_string),
            Some(Retrieval::Lexical),
        )
    }

    fn evidence_of(case: &Case, observed: Observed) -> Evidence {
        evidence_with(case, observed, PROMPT, Some("ok"))
    }

    fn record(turn_id: &str, step: u64, prompt: &str) -> PromptRecord {
        PromptRecord {
            turn_id: turn_id.into(),
            step,
            system_prompt: prompt.into(),
        }
    }

    fn award(evaluation: &ObjectiveEvaluation, id: &str) -> Option<u8> {
        evaluation
            .awards
            .iter()
            .find(|award| award.id == id)
            .unwrap_or_else(|| panic!("no award {id}"))
            .awarded
    }

    /// The system prompt a session with the case's rules sees.
    fn rules_prompt(case: &Case) -> String {
        let rules: String = case
            .rules
            .iter()
            .map(|rule| format!("\n## {}\n{}\n", rule.name, render(rule.content, RUN)))
            .collect();
        format!("{PROMPT}{HEADER}\nThe rules below are the user's standing instructions.\n{rules}")
    }

    #[test]
    fn seeds_and_rules_render_run_scoped_canaries() {
        assert_ne!(render("x-{deploy}", "a"), render("x-{deploy}", "b"));
        assert_eq!(
            render("x-{deploy}", "a"),
            format!("x-{}", canary("a", "deploy"))
        );
        let value = canary("a", "deploy");
        assert_eq!(value.len(), 8);
        assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn every_case_weighs_one_hundred_and_has_a_gate() {
        for case in &CASES {
            let weight: u32 = case.criteria.iter().map(|c| u32::from(c.weight)).sum();
            assert_eq!(weight, 100, "{}", case.id);
            assert!(
                case.criteria.iter().any(|c| c.gate),
                "{} has no gate",
                case.id
            );
            assert_eq!(criteria(case).len(), case.criteria.len(), "{}", case.id);
        }
    }

    #[test]
    fn evidence_carries_the_bank_the_canaries_and_the_planted_flags() {
        let empty = json!({"messages": []});
        let recall = evidence(
            case_named("memory_relevant_recall"),
            RUN,
            &empty,
            &[],
            None,
            Some(Retrieval::Lexical),
        );
        assert_eq!(recall.schema, "memory-evaluation/1");
        assert_eq!(recall.bank, "e2e-a");
        assert_eq!(
            serde_json::to_value(&recall).unwrap()["retrieval"],
            "lexical"
        );
        assert_eq!(
            recall.canaries.iter().collect::<Vec<_>>(),
            [
                (&"channel".to_string(), &canary(RUN, "channel")),
                (&"deploy".to_string(), &canary(RUN, "deploy")),
            ]
        );
        assert_eq!(recall.seeds.len(), DISTRACTORS.len() + 2);
        assert!(recall.seeds.iter().all(|seed| seed.planted));
        assert_eq!(
            recall.seeds[DISTRACTORS.len()].text,
            line(case_named("memory_relevant_recall"), 0)
        );
        assert_eq!(recall.seeds[DISTRACTORS.len()].needed_on, [0]);

        let rules = evidence(
            case_named("memory_value_preference"),
            RUN,
            &empty,
            &[],
            None,
            None,
        );
        assert!(rules.rules.iter().all(|rule| rule.planted));
        assert!(rules.seeds.iter().all(|seed| !seed.planted));
        let control = evidence(
            case_named("memory_value_preference_baseline"),
            RUN,
            &empty,
            &[],
            None,
            None,
        );
        assert!(control.rules.iter().all(|rule| !rule.planted));
        assert!(control.rules[0].content.contains(&canary(RUN, "prefix")));
    }

    #[test]
    fn a_prompt_is_recorded_by_turn_and_matches_by_sha() {
        let sent = format!("{PROMPT}\n\n# Memory — bank: e2e-a\nbody");
        let assistant = |turn: &str| {
            json!({"entry_id": format!("e_{turn}_0_assistant"),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}]},
                   "origin": {"turn_id": turn, "memory_bank": "e2e-a",
                              "req": {"system_sha": system_sha(&sent)}}})
        };
        let transcript = json!({"messages": [assistant("t_a"), assistant("t_b")]});
        let records = [
            record("t_a", 1, "a later step"),
            record("t_a", 0, "another hook's prompt"),
            record("t_a", 0, &sent),
            record("t_b", 1, &sent),
        ];
        let found = evidence(
            case_named("memory_small_talk"),
            RUN,
            &transcript,
            &records,
            None,
            None,
        );
        assert_eq!(found.prompts.len(), 2);
        assert_eq!(
            found.prompts[0],
            ExchangePrompt {
                recorded: true,
                sha_matches: Some(true),
                memory_section: Some("\n\n# Memory — bank: e2e-a\nbody".into()),
            }
        );
        assert_eq!(
            found.prompts[1],
            ExchangePrompt {
                recorded: false,
                sha_matches: Some(false),
                memory_section: None,
            }
        );
    }

    #[test]
    fn an_unstamped_step_trusts_the_prompt_the_observer_recorded() {
        // The origin the published Harness writes: no `req` to hash against.
        let recalled = "<memory bank=\"e2e-a\">\nRelevant remembered memories:\n- The user lives in Lisbon. (2026-10-23)\n</memory>";
        let transcript = json!({"messages": [
            {"entry_id": "e_t_a_0_notice_0",
             "custom": {"custom_type": "model_notice", "data": {"kind": "hook", "text": recalled}}},
            {"entry_id": "e_t_a_0_assistant",
             "message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}]},
             "origin": {"memory_bank": "e2e-a", "memory_banks": ["e2e-a"],
                        "memory_ids": ["fpda904aac412c501d"], "memory_recalled": 1,
                        "turn_id": "t_a"}},
        ]});
        let sent = format!("{PROMPT}{HEADER}");
        let case = case_named("memory_first_person_off_topic");
        let found = evidence(
            case,
            RUN,
            &transcript,
            &[record("t_a", 0, &sent)],
            None,
            Some(Retrieval::Lexical),
        );
        assert_eq!(
            found.prompts,
            [ExchangePrompt {
                recorded: true,
                sha_matches: None,
                memory_section: Some(HEADER.into()),
            }]
        );
        assert_eq!(evaluate(case, &found).infrastructure_error, None);
    }

    #[test]
    fn a_paraphrase_twin_in_one_block_is_a_duplicate() {
        let case = case_named("memory_paraphrase");
        let evidence = evidence_of(
            case,
            observed(&["x"], vec![block(0, vec![line(case, 0), line(case, 1)])]),
        );
        let found = audit(&evidence);
        assert_eq!(found.duplicates, [(0, line(case, 1))]);
        assert_eq!(found.relevant, 1);
        let evaluation = evaluate(case, &evidence);
        assert_eq!(award(&evaluation, "migration_fact_once"), Some(0));
        assert_eq!(award(&evaluation, "migration_fact_injected"), Some(35));
        assert_eq!(evaluation.infrastructure_error, None);
    }

    #[test]
    fn a_fact_still_visible_is_not_missing_and_a_repeat_is_a_duplicate() {
        let case = case_named("memory_session_repeat");
        let (limit, approver) = (line(case, 0), line(case, 1));
        let evidence = evidence_of(
            case,
            observed(
                &["a", "b", "c", "d"],
                vec![
                    block(0, vec![limit.clone()]),
                    block(1, vec![limit.clone(), approver]),
                ],
            ),
        );
        let found = audit(&evidence);
        assert_eq!(found.duplicates, [(1, limit)]);
        assert!(found.missing.is_empty(), "{:?}", found.missing);
        assert!(found.irrelevant.is_empty());
        assert_eq!(found.relevant, 2);
    }

    #[test]
    fn after_compaction_the_needed_fact_must_come_back() {
        let case = case_named("memory_session_repeat");
        let (limit, approver) = (line(case, 0), line(case, 1));
        let compacted = |blocks| {
            let mut seen = observed(&["a", "b", "c", "d"], blocks);
            // Summarizes the exchange-0 block away and keeps the exchange-1 block.
            seen.compactions.push(Compaction {
                position: 25,
                tail_start: Some(5),
            });
            audit(&evidence_of(case, seen))
        };
        let silent = compacted(vec![
            block(0, vec![limit.clone()]),
            block(1, vec![approver.clone()]),
        ]);
        assert_eq!(silent.missing, [(3, "limit".to_string())]);

        let back = compacted(vec![
            block(0, vec![limit.clone()]),
            block(1, vec![approver]),
            block(3, vec![limit]),
        ]);
        assert!(back.missing.is_empty(), "{:?}", back.missing);
        assert!(back.duplicates.is_empty(), "{:?}", back.duplicates);
    }

    #[test]
    fn the_stale_fact_is_irrelevant() {
        let case = case_named("memory_updated_fact");
        let (old, new) = (line(case, 0), line(case, 1));
        let evidence = evidence_of(
            case,
            observed(&["x"], vec![block(0, vec![old.clone(), new])]),
        );
        let found = audit(&evidence);
        assert_eq!(found.irrelevant, [(0, old)]);
        assert_eq!(found.relevant, 1);
        let evaluation = evaluate(case, &evidence);
        assert_eq!(award(&evaluation, "current_fact_injected"), Some(35));
        assert_eq!(award(&evaluation, "stale_fact_left_out"), Some(0));
    }

    #[test]
    fn the_stale_gate_ignores_distractors() {
        let case = case_named("memory_updated_fact");
        let (old, new) = (line(case, 0), line(case, 1));
        let score = |lines: Vec<String>| {
            let evidence = evidence_of(case, observed(&["x"], vec![block(0, lines)]));
            award(&evaluate(case, &evidence), "stale_fact_left_out")
        };
        let metabase = "The analytics dashboards are hosted on Metabase.".to_string();
        assert_eq!(score(vec![new.clone(), metabase]), Some(35));
        assert_eq!(score(vec![new, old]), Some(0));
    }

    #[test]
    fn small_talk_injecting_anything_fails_the_gate() {
        let case = case_named("memory_small_talk");
        let answers = ["a", "b", "c", "d"];
        let quiet = evaluate(case, &evidence_of(case, observed(&answers, vec![])));
        assert_eq!(award(&quiet, "no_memory_on_small_talk"), Some(100));

        let noisy = evidence_of(
            case,
            observed(
                &answers,
                vec![
                    block(1, vec![DISTRACTORS[0].to_string()]),
                    block(2, vec!["Not a planted memory.".to_string()]),
                ],
            ),
        );
        assert_eq!(
            audit(&noisy).irrelevant,
            [
                (1, DISTRACTORS[0].to_string()),
                (2, "Not a planted memory.".to_string())
            ]
        );
        assert_eq!(
            award(&evaluate(case, &noisy), "no_memory_on_small_talk"),
            Some(0)
        );
    }

    #[test]
    fn the_baseline_scores_against_withheld_seeds() {
        let case = case_named("memory_value_project_baseline");
        let evidence = evidence_of(case, observed(&["whatever"], vec![]));
        assert!(evidence.seeds.iter().all(|seed| !seed.planted));
        assert_eq!(audit(&evidence).missing, [(0, "release".to_string())]);
        let evaluation = evaluate(case, &evidence);
        assert_eq!(award(&evaluation, "release_fact_injected"), Some(0));
        assert_eq!(evaluation.infrastructure_error, None);
    }

    #[test]
    fn an_exchange_memory_skipped_is_an_infrastructure_error() {
        let case = case_named("memory_small_talk");
        let mut seen = observed(&["x"], vec![]);
        seen.exchanges[0].memory_bank = None;
        let evaluation = evaluate(case, &evidence_of(case, seen));
        assert!(evaluation.infrastructure_error.is_some());
        assert_eq!(evaluation.awards.len(), case.criteria.len());
    }

    #[test]
    fn an_exchange_memory_read_from_another_bank_is_an_infrastructure_error() {
        let case = case_named("memory_small_talk");
        let answers = ["a", "b", "c", "d"];
        let mut seen = observed(&answers, vec![]);
        seen.exchanges[2].memory_bank = Some("main".into());
        let error = evaluate(case, &evidence_of(case, seen))
            .infrastructure_error
            .expect("a foreign bank is flagged");
        assert!(
            error.contains("main") && error.contains(&bank(RUN)),
            "{error}"
        );

        // A block from another bank is never scored.
        let mut foreign = block(1, vec![DISTRACTORS[0].to_string()]);
        foreign.bank = "main".into();
        let found = audit(&evidence_of(case, observed(&answers, vec![foreign])));
        assert!(found.irrelevant.is_empty(), "{:?}", found.irrelevant);
    }

    #[test]
    fn a_recall_count_without_its_lines_is_an_infrastructure_error() {
        let case = case_named("memory_small_talk");
        let answers = ["a", "b", "c", "d"];
        // Memory says it recalled two, but no block parsed: a quiet pass would be vacuous.
        let mut dropped = observed(&answers, vec![]);
        dropped.exchanges[1].memory_recalled = 2;
        let error = evaluate(case, &evidence_of(case, dropped))
            .infrastructure_error
            .expect("a dropped block is flagged");
        assert!(error.contains("exchange 1"), "{error}");

        let lines = vec![DISTRACTORS[0].to_string(), DISTRACTORS[1].to_string()];
        let matched = observed(&answers, vec![block(1, lines)]);
        assert_eq!(matched.exchanges[1].memory_recalled, 2);
        assert_eq!(
            evaluate(case, &evidence_of(case, matched)).infrastructure_error,
            None
        );
    }

    #[test]
    fn an_unseen_prompt_or_a_failed_compaction_is_an_infrastructure_error() {
        let case = case_named("memory_session_repeat");
        let healthy = evidence_of(case, observed(&["a", "b", "c", "d"], vec![]));
        assert_eq!(evaluate(case, &healthy).infrastructure_error, None);

        let mut unrecorded = healthy.clone();
        unrecorded.prompts[2].recorded = false;
        assert!(evaluate(case, &unrecorded).infrastructure_error.is_some());

        let mut mismatched = healthy.clone();
        mismatched.prompts[1].sha_matches = Some(false);
        assert!(evaluate(case, &mismatched).infrastructure_error.is_some());

        for status in [None, Some("error: boom".to_string())] {
            let mut failed = healthy.clone();
            failed.compaction_status = status;
            assert!(evaluate(case, &failed).infrastructure_error.is_some());
        }

        // A case that forces no compaction needs no status.
        let small_talk = case_named("memory_small_talk");
        let plain = evidence_with(
            small_talk,
            observed(&["x", "y", "z", "w"], vec![]),
            PROMPT,
            None,
        );
        assert_eq!(evaluate(small_talk, &plain).infrastructure_error, None);
    }

    #[test]
    fn a_short_observation_is_an_infrastructure_error_not_a_pass() {
        for id in ["memory_small_talk", "memory_first_person_off_topic"] {
            let case = case_named(id);
            let sent = 1 + case.followups.len();
            for seen in [0, 1, sent - 1, sent + 1]
                .into_iter()
                .filter(|seen| *seen != sent)
            {
                let answers = vec!["x"; seen];
                let evidence = evidence_with(case, observed(&answers, vec![]), PROMPT, None);
                let error = evaluate(case, &evidence)
                    .infrastructure_error
                    .unwrap_or_else(|| panic!("{id}: {seen} of {sent} exchanges went unflagged"));
                assert!(error.contains("exchanges"), "{id}: {error}");
            }
            let whole = evidence_with(case, observed(&vec!["x"; sent], vec![]), PROMPT, None);
            assert_eq!(evaluate(case, &whole).infrastructure_error, None, "{id}");
        }
    }

    #[test]
    fn rules_in_prompt_needs_every_rule_untruncated() {
        let case = case_named("memory_rule_fit");
        let score = |prompt: &str| {
            let evidence = evidence_with(case, observed(&["x"], vec![]), prompt, Some("ok"));
            award(&evaluate(case, &evidence), "rules_in_system_prompt")
        };
        let prompt = rules_prompt(case);
        assert_eq!(score(&prompt), Some(30));
        assert_eq!(
            score(&format!("{prompt}[rule truncated: over the budget]\n")),
            Some(0)
        );
        let one_rule = format!(
            "{PROMPT}{HEADER}\n## {}\n{}\n",
            case.rules[0].name,
            render(case.rules[0].content, RUN)
        );
        assert_eq!(score(&one_rule), Some(0));
        assert_eq!(score(PROMPT), Some(0));
    }

    #[test]
    fn answers_are_read_for_the_runs_canaries() {
        let case = case_named("memory_value_preference");
        let tag = canary(RUN, "prefix");
        let score = |answers: &[&str]| {
            let evidence = evidence_of(case, observed(answers, vec![]));
            award(&evaluate(case, &evidence), "every_message_has_prefix")
        };
        let (first, second) = (format!("[{tag}] Fix crash"), format!("[{tag}] Add toggle"));
        assert_eq!(score(&[&first, &second]), Some(70));
        assert_eq!(score(&[&first, "Add toggle"]), Some(0));
        assert_eq!(score(&[&first]), Some(0));
        let other_run = format!("[{}] Fix crash", canary("b", "prefix"));
        assert_eq!(score(&[&other_run, &second]), Some(0));

        let repeat = case_named("memory_session_repeat");
        let file = format!("It is limits-{}.toml.", canary(RUN, "limits"));
        let after = |answers: &[&str]| {
            let evidence = evidence_of(repeat, observed(answers, vec![]));
            award(
                &evaluate(repeat, &evidence),
                "answer_after_compaction_has_file",
            )
        };
        assert_eq!(after(&["a", "b", "c", &file]), Some(20));
        assert_eq!(after(&["a", "b", "c"]), Some(0));
    }

    #[test]
    fn a_leaked_rule_canary_fails_the_unrelated_rule_check() {
        let case = case_named("memory_rule_fit");
        let score = |answers: &[&str]| {
            let evidence = evidence_of(case, observed(answers, vec![]));
            award(&evaluate(case, &evidence), "unrelated_rule_ignored")
        };
        assert_eq!(score(&["SELECT 1;"]), Some(30));
        assert_eq!(
            score(&[&format!("SELECT 1; {}", canary(RUN, "haiku"))]),
            Some(0)
        );
        // No answer at all is not evidence that the rule was left out.
        assert_eq!(score(&[]), Some(0));
    }

    #[test]
    fn measurements_are_seven_and_counted_in_tokens() {
        let case = case_named("memory_relevant_recall");
        let mut first = block(0, vec![line(case, 0), line(case, 1)]);
        first.memory_bytes = 9;
        first.reminder_bytes = 5;
        let mut second = block(1, Vec::new());
        second.memory_bytes = 10;
        second.reminder_bytes = 6;
        let mut seen = observed(&["a", "b"], vec![first, second]);
        seen.exchanges[1].memory_bank = None;
        let prompt = format!("{PROMPT}{HEADER}body");
        let evidence = evidence_with(case, seen, &prompt, Some("ok"));

        let ids: Vec<&str> = evidence
            .measurements
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "memory_expected_injected",
                "memory_irrelevant_injected",
                "memory_duplicate_injections",
                "memory_injected_tokens",
                "memory_turns_without_hook",
                "memory_rules_tokens",
                "memory_reminder_tokens",
            ]
        );
        let measured = |id: &str| {
            let found = evidence.measurements.iter().find(|m| m.id == id).unwrap();
            (found.value, found.unit.as_str())
        };
        assert_eq!(measured("memory_expected_injected"), (2.0, "count"));
        assert_eq!(measured("memory_irrelevant_injected"), (0.0, "count"));
        assert_eq!(measured("memory_duplicate_injections"), (0.0, "count"));
        // Rounded up once over the sum: (9 + 10 + 3) / 4, not 3 + 3.
        assert_eq!(measured("memory_injected_tokens"), (5.0, "tokens"));
        assert_eq!(measured("memory_turns_without_hook"), (1.0, "count"));
        let section = HEADER.len() + "body".len();
        assert_eq!(
            measured("memory_rules_tokens"),
            (section.div_ceil(4) as f64, "tokens")
        );
        assert_eq!(measured("memory_reminder_tokens"), (3.0, "tokens"));
    }

    #[test]
    fn memory_section_starts_at_its_header() {
        assert_eq!(
            memory_section("base\n\n# Memory — bank: x\nrest"),
            Some("\n\n# Memory — bank: x\nrest")
        );
        assert_eq!(memory_section("base only"), None);
    }

    #[test]
    fn system_sha_hashes_the_json_string() {
        assert_eq!(system_sha("a"), format!("{:x}", Sha256::digest(b"\"a\"")));
    }
}
