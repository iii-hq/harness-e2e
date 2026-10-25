//! Judges a memory session: what was injected where, against what each
//! exchange needed, and what the subject answered. Deterministic: no model.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::cases::{
    audits_bank_after, bank, canary, labels, learned_labels, render, trashed_after, Case, Change,
    Check, Criterion, DISTRACTORS,
};
use super::observe::{observe, visible_at, MemoryBlock, Observed, RulesUpdate};
use crate::report::EvaluationDimension;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::{CriterionSpec, ObjectiveEvaluation};

const SCHEMA: &str = "memory-evaluation/1";
/// Where memory's section starts in the system prompt.
const SECTION_START: &str = "\n\n# Memory — bank: ";
const RULE_TRUNCATED: &str = "[rule truncated";
/// Where the rules start in memory's section, after its fixed header.
const RULES_START: &str = "\nThe rules below are the user's standing instructions.";
/// What the rules update says once no memory section applies, as after a trash.
const NO_SECTION: &str = "No memory section applies now.";
/// Stands in for the text of another bank in the evidence.
const WITHHELD: &str = "[withheld: another bank's text]";
/// The rule memory's extraction appends the instructions it learns to.
pub const LEARNED_RULE: &str = "learned";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedEvidence {
    pub text: String,
    pub group: String,
    pub needed_on: Vec<usize>,
    pub related_on: Vec<usize>,
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

/// What one change between exchanges returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeOutcome {
    /// The exchange it ran after.
    pub after: usize,
    pub function: String,
    /// "ok", or "error: " and why.
    pub status: String,
}

/// One memory of a bank, as `memory::list` returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMemory {
    pub id: String,
    pub text: String,
    /// The session memory's extraction learned it from; none for a saved one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// False once superseded or deleted.
    pub live: bool,
}

/// A bank once memory's extraction had read a session through its last
/// message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extraction {
    /// The session extraction read.
    pub session_id: String,
    /// How long the wait for extraction took.
    pub waited_ms: u64,
    /// Every memory of the bank, superseded ones included.
    pub memories: Vec<StoredMemory>,
    /// Rule name -> content.
    pub rules: BTreeMap<String, String>,
}

impl Extraction {
    /// The live memories extraction learned from the session it read.
    pub fn learned(&self) -> impl Iterator<Item = &StoredMemory> {
        self.memories
            .iter()
            .filter(|memory| memory.live && memory.session_id.as_deref() == Some(&self.session_id))
    }

    /// The instructions the bank's `learned` rule holds, one per line.
    pub fn learned_lines(&self) -> impl Iterator<Item = &str> {
        self.rules
            .get(LEARNED_RULE)
            .into_iter()
            .flat_map(|content| content.lines())
            .filter(|line| line.starts_with("- "))
    }
}

/// What `capture` reads of an attempt.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub prompts: Vec<PromptRecord>,
    pub compaction: Option<String>,
    pub retrieval: Option<Retrieval>,
    /// Memory's stored text of each case seed, `None` until saved.
    pub stored: Vec<Option<String>>,
    pub changes: Vec<ChangeOutcome>,
    /// The bank once extraction read the teacher session.
    pub teacher: Option<Extraction>,
    /// The bank once extraction read the measured session.
    pub bank_after: Option<Extraction>,
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
    /// What each change between exchanges returned, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<ChangeOutcome>,
    /// The bank once memory's extraction read the teacher session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teacher: Option<Extraction>,
    /// The bank once memory's extraction read the measured session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank_after: Option<Extraction>,
    pub measurements: Vec<Measurement>,
}

/// Lines are `(exchange, text)`; `missing` is `(exchange, group)`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Audit {
    /// A group an exchange needs that is neither injected there nor still visible.
    pub missing: Vec<(usize, String)>,
    /// Injected lines their exchange neither needs nor relates to, or that no
    /// seed names.
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

pub fn evidence(case: &Case, run_id: &str, transcript: &Value, snapshot: Snapshot) -> Evidence {
    assemble(case, run_id, observe(transcript), snapshot)
}

/// The bank a memory section or rules update names, if it names one.
fn named_bank(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once(SECTION_START)?;
    Some(rest.split_once('\n').map_or(rest, |(bank, _)| bank))
}

fn assemble(case: &Case, run_id: &str, mut observed: Observed, snapshot: Snapshot) -> Evidence {
    let own = bank(run_id);
    let foreign = |text: &str| named_bank(text).is_some_and(|named| named != own);
    // The evidence is persisted: a fallback to the user's own bank must not
    // copy their memories or rules into it. Blocks keep their line count.
    for block in observed.blocks.iter_mut().filter(|block| block.bank != own) {
        block.lines.fill(WITHHELD.to_string());
    }
    for update in observed.rules_updates.iter_mut() {
        if foreign(&update.text) {
            update.text = WITHHELD.to_string();
        }
    }
    let records = &snapshot.prompts;
    let planted = !case.baseline;
    let distractors = DISTRACTORS.iter().map(|text| SeedEvidence {
        text: (*text).to_string(),
        group: "distractor".to_string(),
        needed_on: Vec::new(),
        related_on: Vec::new(),
        planted: planted && case.distractors,
    });
    let memories = case
        .memories
        .iter()
        .enumerate()
        .map(|(index, seed)| SeedEvidence {
            // Lines are matched to seeds by text: memory's own, once it stored one.
            text: snapshot
                .stored
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or_else(|| render(seed.text, run_id)),
            group: seed.group.to_string(),
            needed_on: seed.needed_on.to_vec(),
            related_on: seed.related_on.to_vec(),
            planted,
        });
    // What the teacher taught may reach any exchange. A memory holding a
    // label the case checks was learned is that label's group.
    let canaries: Vec<(&str, String)> = learned_labels(case)
        .map(|label| (label, canary(run_id, label)))
        .collect();
    let learned: Vec<SeedEvidence> = snapshot
        .teacher
        .iter()
        .flat_map(Extraction::learned)
        .map(|memory| SeedEvidence {
            // As memory renders it in a block: whitespace collapsed.
            text: memory.text.split_whitespace().collect::<Vec<_>>().join(" "),
            group: canaries
                .iter()
                .find(|(_, value)| memory.text.contains(value.as_str()))
                .map_or("extracted", |(label, _)| label)
                .to_string(),
            needed_on: Vec::new(),
            related_on: (0..=case.followups.len()).collect(),
            planted: true,
        })
        .collect();
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
                    .map(|section| if foreign(section) { WITHHELD } else { section }.to_string()),
            }
        })
        .collect();
    let mut evidence = Evidence {
        schema: SCHEMA.to_string(),
        bank: own.clone(),
        retrieval: snapshot.retrieval,
        canaries: labels(case)
            .into_iter()
            .map(|label| (label.to_string(), canary(run_id, label)))
            .collect(),
        seeds: distractors.chain(memories).chain(learned).collect(),
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
        compaction_status: snapshot.compaction,
        changes: snapshot.changes,
        teacher: snapshot.teacher,
        bank_after: snapshot.bank_after,
        measurements: Vec::new(),
    };
    evidence.measurements = measurements(case, &evidence);
    evidence
}

pub fn audit(evidence: &Evidence) -> Audit {
    // Another bank's block is never audited as the attempt's: before a trash
    // it comes with an infrastructure error, after it `BankGoneFrom` fails.
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
            let seed = seed_of(line);
            let needed = seed.is_some_and(|seed| seed.needed_on.contains(&exchange));
            let related = seed.is_some_and(|seed| seed.related_on.contains(&exchange));
            let Some(seed) = seed.filter(|_| needed || related) else {
                audit.irrelevant.push((exchange, line.clone()));
                continue;
            };
            let repeated = block.lines[..index]
                .iter()
                .any(|before| seed_of(before).is_some_and(|other| other.group == seed.group))
                || earlier.iter().any(|before| holds(before, &seed.group));
            // A related line is allowed, so it counts only when it repeats.
            if repeated {
                audit.duplicates.push((exchange, line.clone()));
            } else if needed {
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

fn measurements(case: &Case, evidence: &Evidence) -> Vec<Measurement> {
    let audit = audit(evidence);
    let observed = &evidence.observed;
    let trashed = trashed_after(case);
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
    let mut measured = vec![
        count("memory_expected_injected", audit.relevant),
        count("memory_irrelevant_injected", audit.irrelevant.len()),
        count("memory_duplicate_injections", audit.duplicates.len()),
        tokens(
            "memory_injected_tokens",
            observed.blocks.iter().map(|block| block.memory_bytes).sum(),
        ),
        // A turn after the case trashed its bank has no bank by design.
        count(
            "memory_turns_without_hook",
            observed
                .exchanges
                .iter()
                .enumerate()
                .filter(|(index, exchange)| {
                    exchange.memory_bank.is_none() && trashed.is_none_or(|after| *index <= after)
                })
                .count(),
        ),
        tokens(
            "memory_rules_tokens",
            first_section(evidence)
                .and_then(|section| section.find(RULES_START).map(|start| section.len() - start))
                .unwrap_or(0),
        ),
        tokens(
            "memory_reminder_tokens",
            observed
                .blocks
                .iter()
                .map(|block| block.reminder_bytes)
                .sum(),
        ),
    ];
    // Only a case that moves the rules section measures its updates, so every
    // other case's evidence stays as it was.
    let moves_rules = case
        .changes
        .iter()
        .any(|(_, change)| matches!(change, Change::RuleSet { .. } | Change::DeleteBank));
    if moves_rules {
        measured.push(count("memory_rules_updates", observed.rules_updates.len()));
    }
    // Teacher and extraction run outside the subject's session and cost cap,
    // so what they did is recorded here.
    if let Some(read) = evidence.teacher.as_ref().or(evidence.bank_after.as_ref()) {
        measured.push(count(
            "memory_learned",
            read.learned().count() + read.learned_lines().count(),
        ));
        // shortcut: memory reports no pass duration, so this is the runner's
        // wait past the turn (usually 0); measure the pass once memory reports it.
        measured.push(Measurement {
            id: "memory_extraction_ms".to_string(),
            value: read.waited_ms as f64,
            unit: "ms".to_string(),
        });
    }
    measured
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
        Check::NeededInjected { exchange } => {
            let missing: Vec<(usize, String)> = audit
                .missing
                .iter()
                .filter(|(index, _)| exchange.is_none_or(|wanted| *index == wanted))
                .cloned()
                .collect();
            verdict(
                &missing,
                "every needed memory reached its exchange",
                "needed but not injected or visible",
            )
        }
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
        Check::UpdateOn {
            exchange,
            holds,
            omits,
        } => update_on(evidence, exchange, holds, omits),
        Check::SectionFrozen => section_frozen(evidence),
        Check::BankGoneFrom { exchange } => bank_gone_from(evidence, exchange),
        Check::Learned { canary } => learned(evidence, canary),
        Check::Reached {
            exchange,
            canary,
            section_only,
        } => reached(evidence, exchange, canary, section_only),
        Check::Stored {
            canary,
            include_superseded,
            want,
        } => stored(evidence, canary, include_superseded, want),
        Check::NowhereContains { canary } => nowhere(evidence, canary),
        Check::Retired { group } => retired(evidence, group),
        Check::BankUnchanged => bank_unchanged(evidence),
        Check::AnswerOmits { exchange, canary } => {
            let Some(value) = evidence.canaries.get(canary) else {
                return (false, format!("no canary recorded for {canary}"));
            };
            let wanted = exchange.map_or(0..=case.followups.len(), |index| index..=index);
            // An exchange that never answered cannot show the value was left out.
            let leaking: Vec<usize> = wanted
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

/// The rules updates memory appended on `exchange`'s turn; `None` when the
/// exchange never answered.
fn updates_on(evidence: &Evidence, exchange: usize) -> Option<Vec<&RulesUpdate>> {
    let turn = &evidence.observed.exchanges.get(exchange)?.turn_id;
    let updates = &evidence.observed.rules_updates;
    Some(updates.iter().filter(|u| &u.turn_id == turn).collect())
}

fn update_on(
    evidence: &Evidence,
    exchange: usize,
    holds: Option<&str>,
    omits: &[&str],
) -> (bool, String) {
    let Some(updates) = updates_on(evidence, exchange) else {
        return (false, format!("exchange {exchange} never answered"));
    };
    // None is a change that never arrived; two is one change sent twice.
    let [update] = updates[..] else {
        return (
            false,
            format!(
                "exchange {exchange} carries {} rules updates, not one",
                updates.len()
            ),
        );
    };
    let wanted = holds.map(|label| (label, true));
    let unwanted = omits.iter().map(|label| (*label, false));
    let mut problems = Vec::new();
    for (label, held) in wanted.into_iter().chain(unwanted) {
        match evidence.canaries.get(label) {
            None => problems.push(format!("no canary recorded for {label}")),
            Some(value) if update.text.contains(value.as_str()) != held => problems.push(format!(
                "the update {} {label} ({value})",
                if held { "lacks" } else { "carries" }
            )),
            Some(_) => {}
        }
    }
    if problems.is_empty() {
        (
            true,
            format!("exchange {exchange} carries one rules update, as planned"),
        )
    } else {
        (false, problems.join("; "))
    }
}

fn section_frozen(evidence: &Evidence) -> (bool, String) {
    let Some(first) = first_section(evidence).filter(|section| !section.is_empty()) else {
        return (false, "exchange 0 recorded no memory section".to_string());
    };
    let moved: Vec<usize> = evidence
        .prompts
        .iter()
        .enumerate()
        .filter(|(_, prompt)| prompt.memory_section.as_deref() != Some(first))
        .map(|(index, _)| index)
        .collect();
    if moved.is_empty() {
        (
            true,
            "every exchange kept exchange 0's memory section".to_string(),
        )
    } else {
        (
            false,
            format!("exchanges whose memory section differs from exchange 0's: {moved:?}"),
        )
    }
}

fn bank_gone_from(evidence: &Evidence, exchange: usize) -> (bool, String) {
    let Some(updates) = updates_on(evidence, exchange) else {
        return (false, format!("exchange {exchange} never answered"));
    };
    let mut problems = Vec::new();
    for (index, turn) in evidence
        .observed
        .exchanges
        .iter()
        .enumerate()
        .skip(exchange)
    {
        if let Some(read) = &turn.memory_bank {
            problems.push(format!("exchange {index}: memory read bank {read}"));
        }
        let blocks = evidence
            .observed
            .blocks
            .iter()
            .filter(|block| block.turn_id == turn.turn_id)
            .count();
        if blocks > 0 {
            problems.push(format!("exchange {index}: {blocks} memory blocks injected"));
        }
    }
    // The note is what shows memory ran and chose no memory.
    if !updates
        .iter()
        .any(|update| update.text.contains(NO_SECTION))
    {
        problems.push(format!(
            "exchange {exchange} carries no update saying {NO_SECTION:?}"
        ));
    }
    if problems.is_empty() {
        (
            true,
            format!("from exchange {exchange} on, memory read no bank and said so"),
        )
    } else {
        (false, problems.join("; "))
    }
}

/// The value of the canary `label`, or the details of a check that cannot read it.
fn value_of<'a>(evidence: &'a Evidence, label: &str) -> Result<&'a str, (bool, String)> {
    evidence
        .canaries
        .get(label)
        .map(String::as_str)
        .ok_or_else(|| (false, format!("no canary recorded for {label}")))
}

/// The bank after the teacher's extraction, or the details of a check that
/// has none to read.
fn teacher_read(evidence: &Evidence) -> Result<&Extraction, (bool, String)> {
    evidence.teacher.as_ref().ok_or_else(|| {
        (
            false,
            "the bank was never read after the teacher".to_string(),
        )
    })
}

fn learned(evidence: &Evidence, label: &str) -> (bool, String) {
    let (value, teacher) = match (value_of(evidence, label), teacher_read(evidence)) {
        (Ok(value), Ok(teacher)) => (value, teacher),
        (Err(missing), _) | (_, Err(missing)) => return missing,
    };
    if teacher.learned().any(|memory| memory.text.contains(value)) {
        (true, format!("learned {label} ({value}) as a memory"))
    } else if teacher.learned_lines().any(|line| line.contains(value)) {
        (true, format!("learned {label} ({value}) as a rule"))
    } else {
        (
            false,
            format!(
                "extraction kept {} memories and {} rule lines from the teacher, none with {label} ({value})",
                teacher.learned().count(),
                teacher.learned_lines().count()
            ),
        )
    }
}

fn reached(
    evidence: &Evidence,
    exchange: usize,
    label: &str,
    section_only: bool,
) -> (bool, String) {
    let value = match value_of(evidence, label) {
        Ok(value) => value,
        Err(missing) => return missing,
    };
    let Some(turn) = evidence.observed.exchanges.get(exchange) else {
        return (false, format!("exchange {exchange} never answered"));
    };
    let in_section = evidence
        .prompts
        .get(exchange)
        .and_then(|prompt| prompt.memory_section.as_deref())
        .is_some_and(|section| section.contains(value));
    let injected = !section_only
        && evidence
            .observed
            .blocks
            .iter()
            .filter(|block| block.turn_id == turn.turn_id)
            .any(|block| block.lines.iter().any(|line| line.contains(value)));
    match (in_section, injected) {
        (true, _) => (
            true,
            format!("exchange {exchange}'s memory section holds {label}"),
        ),
        (_, true) => (
            true,
            format!("a memory injected on exchange {exchange} holds {label}"),
        ),
        _ if section_only => (
            false,
            format!("exchange {exchange}'s memory section lacks {label} ({value})"),
        ),
        _ => (
            false,
            format!("nothing exchange {exchange} got from memory holds {label} ({value})"),
        ),
    }
}

fn stored(
    evidence: &Evidence,
    label: &str,
    include_superseded: bool,
    want: bool,
) -> (bool, String) {
    let (value, teacher) = match (value_of(evidence, label), teacher_read(evidence)) {
        (Ok(value), Ok(teacher)) => (value, teacher),
        (Err(missing), _) | (_, Err(missing)) => return missing,
    };
    let memories = teacher
        .memories
        .iter()
        .filter(|memory| include_superseded || memory.live)
        .filter(|memory| memory.text.contains(value))
        .map(|memory| format!("memory {}", memory.id));
    let rules = teacher
        .rules
        .iter()
        .filter(|(_, content)| content.contains(value))
        .map(|(name, _)| format!("rule {name}"));
    let holders: Vec<String> = memories.chain(rules).collect();
    let details = if holders.is_empty() {
        format!("nothing in the bank holds {label} ({value})")
    } else {
        format!("{label} ({value}) is stored in: {}", holders.join(", "))
    };
    (holders.is_empty() != want, details)
}

fn nowhere(evidence: &Evidence, label: &str) -> (bool, String) {
    let value = match value_of(evidence, label) {
        Ok(value) => value,
        Err(missing) => return missing,
    };
    let observed = &evidence.observed;
    let answers = observed
        .exchanges
        .iter()
        .enumerate()
        .filter(|(_, exchange)| exchange.answer.contains(value))
        .map(|(index, _)| format!("the answer of exchange {index}"));
    let blocks = observed
        .blocks
        .iter()
        .filter(|block| block.lines.iter().any(|line| line.contains(value)))
        .map(|block| format!("a memory injected on turn {}", block.turn_id));
    let sections = evidence
        .prompts
        .iter()
        .enumerate()
        .filter(|(_, prompt)| {
            prompt
                .memory_section
                .as_deref()
                .is_some_and(|section| section.contains(value))
        })
        .map(|(index, _)| format!("the memory section of exchange {index}"));
    let updates = observed
        .rules_updates
        .iter()
        .filter(|update| update.text.contains(value))
        .map(|update| format!("a rules update on turn {}", update.turn_id));
    let found: Vec<String> = answers
        .chain(blocks)
        .chain(sections)
        .chain(updates)
        .collect();
    if found.is_empty() {
        (
            true,
            format!("nothing memory sent or the subject answered holds {label}"),
        )
    } else {
        (
            false,
            format!("{label} ({value}) is in {}", found.join(", ")),
        )
    }
}

fn retired(evidence: &Evidence, group: &str) -> (bool, String) {
    let teacher = match teacher_read(evidence) {
        Ok(teacher) => teacher,
        Err(missing) => return missing,
    };
    let live: Vec<&str> = evidence
        .seeds
        .iter()
        .filter(|seed| seed.group == group)
        .filter(|seed| {
            teacher
                .memories
                .iter()
                .any(|memory| memory.live && memory.text == seed.text)
        })
        .map(|seed| seed.text.as_str())
        .collect();
    if live.is_empty() {
        (true, format!("no {group} memory is live in the bank"))
    } else {
        (
            false,
            format!("{group} memories still live: {}", live.join("; ")),
        )
    }
}

fn bank_unchanged(evidence: &Evidence) -> (bool, String) {
    let Some(after) = &evidence.bank_after else {
        return (
            false,
            "the bank was never read after the session".to_string(),
        );
    };
    let planted = |text: &str| {
        evidence
            .seeds
            .iter()
            .any(|seed| seed.planted && seed.text == text)
    };
    let memories = after
        .memories
        .iter()
        .filter(|memory| !(memory.live && planted(&memory.text)))
        .map(|memory| format!("memory {:?}", memory.text));
    let rules = after
        .rules
        .iter()
        .filter(|(name, content)| {
            !evidence
                .rules
                .iter()
                .any(|rule| rule.planted && rule.name == **name && rule.content == **content)
        })
        .map(|(name, _)| format!("rule {name}"));
    let changed: Vec<String> = memories.chain(rules).collect();
    if changed.is_empty() {
        (
            true,
            "the bank holds what setup planted, and nothing else".to_string(),
        )
    } else {
        (
            false,
            format!("memory changed the bank: {}", changed.join("; ")),
        )
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
    let trashed = trashed_after(case);
    for (index, exchange) in evidence.observed.exchanges.iter().enumerate() {
        // A bank other than the attempt's means the turn-metadata pin never
        // reached memory: it read `main` or a directory bank instead. Once the
        // case trashed its bank, no bank or another one is what its gate
        // scores: a fallback to `main` is a product failure, not a rerun.
        // shortcut: a hook that never ran after the trash fails `BankGoneFrom`
        // instead of rerunning; tell it from one that ran by the exchange's
        // recorded memory section once a flaky hook shows up after a trash.
        match exchange.memory_bank.as_deref() {
            _ if trashed.is_some_and(|after| index > after) => {}
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
        let updated = updates_on(evidence, index)
            .is_some_and(|updates| updates.iter().any(|update| update.step == 0));
        if exchange.rules_updated && !updated {
            problems.push(format!(
                "exchange {index}: memory says it sent a rules update, none parsed"
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
    // Setup and capture fail when they cannot read the bank, so a missing
    // read is a defect, never a score.
    if !case.teacher.is_empty() && evidence.teacher.is_none() {
        problems.push("the bank was never read after the teacher".to_string());
    }
    if audits_bank_after(case) && evidence.bank_after.is_none() {
        problems.push("the bank was never read after the session".to_string());
    }
    // `before_followup` cannot fail the run, so a change that failed or never
    // ran is caught here.
    if evidence.changes.len() != case.changes.len() {
        problems.push(format!(
            "{} of the case's {} changes ran",
            evidence.changes.len(),
            case.changes.len()
        ));
    }
    for change in evidence
        .changes
        .iter()
        .filter(|change| change.status != "ok")
    {
        problems.push(format!(
            "the {} after exchange {} failed: {}",
            change.function, change.after, change.status
        ));
    }
    (!problems.is_empty()).then(|| problems.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::memory::cases::{
        audits_bank_after, bank, canary, render, Case, CASES, CHANGING, DISTRACTORS,
    };
    use crate::scenarios::memory::observe::{
        Compaction, Exchange, MemoryBlock, Observed, RulesUpdate,
    };
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
            rules_updated: false,
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
            rules_updates: Vec::new(),
        }
    }

    /// Every change of the case ran, and ran fine.
    fn applied(case: &Case) -> Vec<ChangeOutcome> {
        case.changes
            .iter()
            .map(|&(after, change)| ChangeOutcome {
                after,
                function: change.function().into(),
                status: "ok".into(),
            })
            .collect()
    }

    /// Every exchange saw `prompt`, its recorded sha says so, and every
    /// change of the case ran fine.
    fn recorded(
        case: &Case,
        observed: &mut Observed,
        prompt: &str,
        status: Option<&str>,
    ) -> Snapshot {
        for exchange in &mut observed.exchanges {
            exchange.system_sha = Some(system_sha(prompt));
        }
        let records: Vec<PromptRecord> = observed
            .exchanges
            .iter()
            .map(|exchange| record(&exchange.turn_id, 0, prompt))
            .collect();
        Snapshot {
            prompts: records,
            compaction: status.map(str::to_string),
            retrieval: Some(Retrieval::Lexical),
            changes: applied(case),
            ..Snapshot::default()
        }
    }

    fn evidence_with(
        case: &Case,
        mut observed: Observed,
        prompt: &str,
        status: Option<&str>,
    ) -> Evidence {
        let snapshot = recorded(case, &mut observed, prompt, status);
        assemble(case, RUN, observed, snapshot)
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
        let lexical = Snapshot {
            retrieval: Some(Retrieval::Lexical),
            ..Snapshot::default()
        };
        let recall = evidence(case_named("memory_relevant_recall"), RUN, &empty, lexical);
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
            Snapshot::default(),
        );
        assert!(rules.rules.iter().all(|rule| rule.planted));
        assert!(rules.seeds.iter().all(|seed| !seed.planted));
        let control = evidence(
            case_named("memory_value_preference_baseline"),
            RUN,
            &empty,
            Snapshot::default(),
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
        let recorded = Snapshot {
            prompts: records.to_vec(),
            ..Snapshot::default()
        };
        let found = evidence(case_named("memory_small_talk"), RUN, &transcript, recorded);
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
        let recorded = Snapshot {
            prompts: vec![record("t_a", 0, &sent)],
            retrieval: Some(Retrieval::Lexical),
            ..Snapshot::default()
        };
        let found = evidence(case, RUN, &transcript, recorded);
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
    fn a_related_fact_is_neither_required_nor_irrelevant() {
        let case = case_named("memory_session_repeat");
        let (limit, approver) = (line(case, 0), line(case, 1));
        let answers = ["a", "b", "c", "d"];
        // The approver shares the subject of exchange 0 without being needed there.
        let found = audit(&evidence_of(
            case,
            observed(
                &answers,
                vec![
                    block(0, vec![limit.clone(), approver.clone()]),
                    block(3, vec![approver.clone()]),
                ],
            ),
        ));
        assert!(found.irrelevant.is_empty(), "{:?}", found.irrelevant);
        assert_eq!(found.relevant, 1);
        // Still a repeat while its earlier line is visible.
        assert_eq!(found.duplicates, [(3, approver)]);
        assert!(found.missing.is_empty(), "{:?}", found.missing);

        // Related exchanges are never required.
        let found = audit(&evidence_of(
            case,
            observed(&answers, vec![block(0, vec![limit])]),
        ));
        assert_eq!(
            found.missing,
            [(1, "approver".to_string()), (2, "approver".to_string())]
        );
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
        assert_eq!(evaluation.awards.len(), 1);
        assert_eq!(award(&evaluation, "answer_uses_release_command"), Some(0));
        assert_eq!(evaluation.infrastructure_error, None);
        // The planted value without memory completes it: the redesign signal.
        let guessed = format!("release-{} --channel stable", canary(RUN, "release"));
        let evaluation = evaluate(case, &evidence_of(case, observed(&[&guessed], vec![])));
        assert_eq!(award(&evaluation, "answer_uses_release_command"), Some(100));
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
        // A section with no rules: memory's fixed header alone counts nothing.
        assert_eq!(measured("memory_rules_tokens"), (0.0, "tokens"));
        assert_eq!(measured("memory_reminder_tokens"), (3.0, "tokens"));
    }

    #[test]
    fn memory_rules_tokens_count_the_rules_only() {
        let rules_tokens = |case: &Case, prompt: &str| {
            let evidence = evidence_with(case, observed(&["x"], vec![]), prompt, Some("ok"));
            let found = evidence
                .measurements
                .iter()
                .find(|m| m.id == "memory_rules_tokens");
            found.unwrap().value
        };
        let header_only = format!("{PROMPT}{HEADER}");
        assert_eq!(
            rules_tokens(case_named("memory_relevant_recall"), &header_only),
            0.0
        );
        let case = case_named("memory_value_preference");
        let prompt = rules_prompt(case);
        let rules = &prompt[prompt.find("\nThe rules below").unwrap()..];
        assert!(rules_tokens(case, &prompt) > 0.0);
        assert_eq!(rules_tokens(case, &prompt), rules.len().div_ceil(4) as f64);
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

    /// The rules update memory appends on exchange `index`'s opening step.
    fn update(index: usize, section: &str) -> RulesUpdate {
        RulesUpdate {
            turn_id: format!("t{index}"),
            step: 0,
            position: 10 * index,
            text: format!(
                "<memory update=\"rules\">\nThe memory rules changed. This block replaces the memory section of the system prompt and any earlier update:{section}</memory>"
            ),
        }
    }

    fn measured(evidence: &Evidence, id: &str) -> Option<f64> {
        evidence
            .measurements
            .iter()
            .find(|measurement| measurement.id == id)
            .map(|measurement| measurement.value)
    }

    #[test]
    fn a_fallback_after_the_bank_is_trashed_is_scored_not_rerun() {
        let case = &CHANGING;
        let private = "The user's own private fact.".to_string();
        // Memory read the user's `main` on the turn after the trash.
        let mut theirs = block(2, vec![private.clone()]);
        theirs.bank = "main".into();
        let mut seen = observed(&["a", "b", "c"], vec![theirs]);
        seen.exchanges[2].memory_bank = Some("main".into());
        let fallback = evidence_of(case, seen);
        let evaluation = evaluate(case, &fallback);
        assert_eq!(evaluation.infrastructure_error, None);
        assert_eq!(award(&evaluation, "memory_gone_after_trash"), Some(0));
        // The user's own memory never reaches the persisted evidence; its count does.
        let kept = serde_json::to_string(&fallback).unwrap();
        assert!(!kept.contains(&private), "{kept}");
        assert_eq!(fallback.observed.blocks[0].lines.len(), 1);

        // No bank and no note that none applies: scored too.
        let mut silent = observed(&["a", "b", "c"], vec![]);
        silent.exchanges[2].memory_bank = None;
        let evaluation = evaluate(case, &evidence_of(case, silent));
        assert_eq!(evaluation.infrastructure_error, None);
        assert_eq!(award(&evaluation, "memory_gone_after_trash"), Some(0));

        // Before the trash, another bank is still a pin that never reached memory.
        let mut early = observed(&["a", "b", "c"], vec![]);
        early.exchanges[1].memory_bank = Some("main".into());
        early.exchanges[2].memory_bank = None;
        let error = evaluate(case, &evidence_of(case, early))
            .infrastructure_error
            .expect("a foreign bank before the trash is flagged");
        assert!(error.contains("exchange 1"), "{error}");
    }

    #[test]
    fn a_trashed_bank_reads_no_bank_and_says_no_section_applies() {
        let case = &CHANGING;
        let mut seen = observed(&["a", "b", "c"], vec![]);
        seen.exchanges[2].memory_bank = None;
        seen.exchanges[2].rules_updated = true;
        seen.rules_updates
            .push(update(2, "\nNo memory section applies now.\n"));
        let evidence = evidence_of(case, seen);
        let evaluation = evaluate(case, &evidence);
        assert_eq!(evaluation.infrastructure_error, None);
        assert_eq!(award(&evaluation, "memory_gone_after_trash"), Some(30));
        // The hook ran on a bank that is gone: not a turn without the hook.
        assert_eq!(measured(&evidence, "memory_turns_without_hook"), Some(0.0));
        assert_eq!(measured(&evidence, "memory_rules_updates"), Some(1.0));

        // A block after the trash fails it, whichever bank it names.
        let mut after = observed(&["a", "b", "c"], vec![block(2, vec![line(case, 0)])]);
        after.exchanges[2].memory_bank = None;
        after
            .rules_updates
            .push(update(2, "\nNo memory section applies now.\n"));
        let evaluation = evaluate(case, &evidence_of(case, after));
        assert_eq!(award(&evaluation, "memory_gone_after_trash"), Some(0));
    }

    #[test]
    fn an_edited_rule_arrives_as_one_update_on_its_exchange() {
        let case = &CHANGING;
        let rule = |label: &str| {
            format!(
                "{HEADER}\n## commit-messages\nPrefix [{}].\n",
                canary(RUN, label)
            )
        };
        let score = |updates: Vec<RulesUpdate>| {
            let mut seen = observed(&["a", "b", "c"], vec![]);
            seen.rules_updates = updates;
            award(
                &evaluate(case, &evidence_of(case, seen)),
                "edit_sent_as_update",
            )
        };
        assert_eq!(score(vec![update(1, &rule("after"))]), Some(20));
        assert_eq!(score(vec![]), Some(0));
        assert_eq!(score(vec![update(1, &rule("before"))]), Some(0));
        let both = format!("{}{}", rule("after"), rule("before"));
        assert_eq!(score(vec![update(1, &both)]), Some(0));
        let twice = vec![update(1, &rule("after")), update(1, &rule("after"))];
        assert_eq!(score(twice), Some(0));
        assert_eq!(score(vec![update(2, &rule("after"))]), Some(0));
    }

    #[test]
    fn the_memory_section_stays_as_the_first_turn_saw_it() {
        let case = &CHANGING;
        let frozen = format!("{PROMPT}{HEADER}");
        let seen = || observed(&["a", "b", "c"], vec![]);
        let score = |evidence: Evidence| award(&evaluate(case, &evidence), "section_frozen");
        assert_eq!(score(evidence_with(case, seen(), &frozen, None)), Some(10));
        let mut rewritten = evidence_with(case, seen(), &frozen, None);
        rewritten.prompts[1].memory_section = Some(format!("{HEADER}\n## commit-messages\nnew\n"));
        assert_eq!(score(rewritten), Some(0));
        // No section is nothing kept frozen.
        assert_eq!(score(evidence_with(case, seen(), PROMPT, None)), Some(0));
    }

    #[test]
    fn an_exchange_scoped_check_reads_that_exchange_only() {
        let case = &CHANGING;
        let current = vec![line(case, 4), line(case, 5)];
        let evidence = evidence_of(case, observed(&["a", "b", "c"], vec![block(1, current)]));
        // Exchange 0 lacks its fact, and only exchange 1 is judged.
        assert_eq!(audit(&evidence).missing, [(0, "deploy".to_string())]);
        let judged = |evidence: &Evidence| {
            award(&evaluate(case, evidence), "current_injected_after_changes")
        };
        assert_eq!(judged(&evidence), Some(20));
        let half = observed(&["a", "b", "c"], vec![block(1, vec![line(case, 4)])]);
        assert_eq!(judged(&evidence_of(case, half)), Some(0));

        // The old prefix belongs on exchange 0; only exchange 1 must leave it out.
        let old = format!("[{}] Fix crash", canary(RUN, "before"));
        let omits = |answers: &[&str]| {
            let evidence = evidence_of(case, observed(answers, vec![]));
            award(&evaluate(case, &evidence), "old_prefix_dropped")
        };
        assert_eq!(omits(&[&old, "b", "c"]), Some(10));
        assert_eq!(omits(&["a", &old, "c"]), Some(0));
        // An exchange that never answered cannot show the prefix was dropped.
        assert_eq!(omits(&["a"]), Some(0));
    }

    #[test]
    fn a_change_that_failed_or_never_ran_is_an_infrastructure_error() {
        let case = &CHANGING;
        let healthy = evidence_of(case, observed(&["a", "b", "c"], vec![]));
        assert_eq!(evaluate(case, &healthy).infrastructure_error, None);

        let mut failed = healthy.clone();
        failed.changes[2].status = "error: memory::save: timed out".into();
        let error = evaluate(case, &failed)
            .infrastructure_error
            .expect("a failed change is flagged");
        assert!(error.contains("memory::save"), "{error}");

        let mut missing = healthy;
        missing.changes.pop();
        assert!(evaluate(case, &missing).infrastructure_error.is_some());
    }

    #[test]
    fn a_rules_update_memory_announced_must_parse() {
        let case = &CHANGING;
        let mut seen = observed(&["a", "b", "c"], vec![]);
        seen.exchanges[1].rules_updated = true;
        let error = evaluate(case, &evidence_of(case, seen.clone()))
            .infrastructure_error
            .expect("an unparsed update is flagged");
        assert!(error.contains("exchange 1"), "{error}");
        seen.rules_updates.push(update(1, HEADER));
        assert_eq!(
            evaluate(case, &evidence_of(case, seen)).infrastructure_error,
            None
        );
    }

    #[test]
    fn a_seed_is_matched_by_the_text_memory_stored() {
        let case = case_named("memory_relevant_recall");
        let stored = format!("{} Stored.", line(case, 0));
        let snapshot = Snapshot {
            stored: vec![Some(stored.clone()), None],
            ..Snapshot::default()
        };
        let seen = observed(&["x"], vec![block(0, vec![stored.clone(), line(case, 1)])]);
        let evidence = assemble(case, RUN, seen, snapshot);
        assert_eq!(evidence.seeds[DISTRACTORS.len()].text, stored);
        assert_eq!(evidence.seeds[DISTRACTORS.len() + 1].text, line(case, 1));
        let found = audit(&evidence);
        assert!(found.irrelevant.is_empty(), "{:?}", found.irrelevant);
        assert!(found.missing.is_empty(), "{:?}", found.missing);
    }

    #[test]
    fn evidence_keeps_no_text_from_another_bank() {
        let case = &CHANGING;
        let theirs = "\n\n# Memory — bank: main\n## private\nThe user's own rule.\n";
        let mut seen = observed(&["a", "b", "c"], vec![]);
        seen.rules_updates = vec![update(1, &format!("{HEADER}own rule\n")), update(2, theirs)];
        let evidence = evidence_with(case, seen, &format!("{PROMPT}{theirs}"), None);
        let kept = serde_json::to_string(&evidence).unwrap();
        assert!(!kept.contains("The user's own rule."), "{kept}");
        assert!(evidence.observed.rules_updates[0].text.contains("own rule"));
        assert_eq!(evidence.observed.rules_updates.len(), 2);
    }

    /// The case's awards for `blocks`, with one answer per exchange.
    fn awards(case: &Case, blocks: Vec<MemoryBlock>) -> ObjectiveEvaluation {
        let answers = vec!["x"; 1 + case.followups.len()];
        evaluate(case, &evidence_of(case, observed(&answers, blocks)))
    }

    #[test]
    fn chatter_that_brings_user_facts_fails_its_gate() {
        let case = case_named("memory_chatter_after_answer");
        let deploy = block(0, vec![line(case, 0)]);
        let quiet = awards(case, vec![deploy.clone()]);
        assert_eq!(award(&quiet, "deploy_fact_injected"), Some(25));
        assert_eq!(award(&quiet, "no_memory_on_chatter"), Some(60));
        // "tell me more" read as a question about the user: six user facts.
        let user_facts = DISTRACTORS[..6]
            .iter()
            .map(|text| text.to_string())
            .collect();
        let noisy = awards(case, vec![deploy, block(1, user_facts)]);
        assert_eq!(award(&noisy, "no_memory_on_chatter"), Some(0));
        assert_eq!(noisy.infrastructure_error, None);
        // Nothing at all fails the positive control.
        assert_eq!(
            award(&awards(case, vec![]), "deploy_fact_injected"),
            Some(0)
        );
    }

    #[test]
    fn the_user_fact_a_task_needs_must_reach_it_alone() {
        let case = case_named("memory_user_fact_needed");
        let closing = line(case, 0);
        let found = awards(case, vec![block(0, vec![closing.clone()])]);
        assert_eq!(award(&found, "user_fact_injected"), Some(40));
        assert_eq!(award(&found, "no_unrelated_memories"), Some(30));
        assert_eq!(award(&awards(case, vec![]), "user_fact_injected"), Some(0));
        let padded = awards(case, vec![block(0, vec![closing, DISTRACTORS[0].into()])]);
        assert_eq!(award(&padded, "no_unrelated_memories"), Some(0));
    }

    #[test]
    fn about_me_wants_every_user_fact_and_no_workplace_fact() {
        let case = case_named("memory_about_me");
        let user: Vec<String> = (0..case.memories.len())
            .filter(|&index| case.memories[index].group != "workplace")
            .map(|index| line(case, index))
            .collect();
        let workplace = (0..case.memories.len())
            .find(|&index| case.memories[index].group == "workplace")
            .map(|index| line(case, index))
            .unwrap();
        // With six or fewer, memory's six-line cap never picks among them.
        assert!(user.len() <= 6, "{user:?}");
        let all = awards(case, vec![block(0, user.clone())]);
        assert_eq!(award(&all, "user_facts_injected"), Some(40));
        assert_eq!(award(&all, "no_workplace_memories"), Some(30));
        let short = awards(case, vec![block(0, user[1..].to_vec())]);
        assert_eq!(award(&short, "user_facts_injected"), Some(0));
        let padded = awards(case, vec![block(0, [user, vec![workplace]].concat())]);
        assert_eq!(award(&padded, "no_workplace_memories"), Some(0));
    }

    #[test]
    fn a_rewording_sent_while_its_twin_is_visible_is_a_duplicate() {
        let case = case_named("memory_reworded_across_turns");
        let (first, rewording) = (line(case, 0), line(case, 1));
        // Exchange 1 still shows the first wording: nothing more to send.
        let quiet = awards(case, vec![block(0, vec![first.clone()])]);
        assert_eq!(award(&quiet, "migration_fact_present"), Some(30));
        assert_eq!(award(&quiet, "no_rewording_while_visible"), Some(50));
        let reworded = awards(case, vec![block(0, vec![first]), block(1, vec![rewording])]);
        assert_eq!(award(&reworded, "no_rewording_while_visible"), Some(0));
        assert_eq!(award(&reworded, "migration_fact_present"), Some(30));
    }

    #[test]
    fn one_sibling_told_for_both_fails_the_gate() {
        let case = case_named("memory_sibling_facts");
        let (billing, ledger) = (line(case, 0), line(case, 1));
        let both = awards(case, vec![block(0, vec![billing.clone(), ledger])]);
        assert_eq!(award(&both, "both_siblings_injected"), Some(70));
        assert_eq!(award(&both, "no_unrelated_memories"), Some(30));
        let collapsed = awards(case, vec![block(0, vec![billing])]);
        assert_eq!(award(&collapsed, "both_siblings_injected"), Some(0));
    }

    #[test]
    fn a_rule_edit_and_its_removal_each_arrive_as_one_update() {
        let case = case_named("memory_rule_changed_mid_session");
        let edited = format!(
            "{HEADER}\n## commit-messages\nPrefix [{}].\n",
            canary(RUN, "after")
        );
        let run = |updates: Vec<RulesUpdate>, answers: &[&str]| {
            let mut seen = observed(answers, vec![]);
            seen.rules_updates = updates;
            evaluate(case, &evidence_with(case, seen, &rules_prompt(case), None))
        };
        let (before, after) = (
            format!("[{}] Fix crash", canary(RUN, "before")),
            format!("[{}] Add toggle", canary(RUN, "after")),
        );
        let delivered = run(
            vec![update(1, &edited), update(2, HEADER)],
            &[&before, &after, "Bump lodash"],
        );
        assert_eq!(delivered.infrastructure_error, None);
        for (id, weight) in [
            ("edit_sent_as_update", 30),
            ("removal_sent_as_update", 20),
            ("system_prompt_unchanged", 20),
            ("edited_rule_obeyed", 20),
            ("removed_rule_not_applied", 10),
        ] {
            assert_eq!(award(&delivered, id), Some(weight), "{id}");
        }
        // The removal never arrived: the edit is still the last word.
        let stale = run(
            vec![update(1, &edited), update(2, &edited)],
            &[&before, &after, &after],
        );
        assert_eq!(award(&stale, "removal_sent_as_update"), Some(0));
        assert_eq!(award(&stale, "removed_rule_not_applied"), Some(0));
        // The removal brought the original rule back instead.
        let original = format!(
            "{HEADER}\n## commit-messages\nPrefix [{}].\n",
            canary(RUN, "before")
        );
        let reverted = run(
            vec![update(1, &edited), update(2, &original)],
            &[&before, &after, "Bump lodash"],
        );
        assert_eq!(award(&reverted, "removal_sent_as_update"), Some(0));
        let silent = run(vec![], &[&before, &after, "Bump lodash"]);
        assert_eq!(award(&silent, "edit_sent_as_update"), Some(0));
        assert_eq!(award(&silent, "removal_sent_as_update"), Some(0));
    }

    #[test]
    fn bank_edits_reach_the_next_turn_as_they_now_stand() {
        let case = case_named("memory_bank_edits_mid_session");
        let deploy = block(0, vec![line(case, 0)]);
        let current = vec![line(case, 4), line(case, 5), line(case, 6)];
        let fresh = awards(case, vec![deploy.clone(), block(1, current.clone())]);
        assert_eq!(fresh.infrastructure_error, None);
        assert_eq!(award(&fresh, "retired_memories_left_out"), Some(35));
        assert_eq!(award(&fresh, "current_memories_injected"), Some(35));
        assert_eq!(award(&fresh, "no_unrelated_memories"), Some(10));
        // Each retired text, deleted, superseded or rewritten, fails the gate.
        for retired in 1..=3 {
            let stale = [current.clone(), vec![line(case, retired)]].concat();
            let evaluation = awards(case, vec![deploy.clone(), block(1, stale)]);
            assert_eq!(
                award(&evaluation, "retired_memories_left_out"),
                Some(0),
                "{retired}"
            );
        }
        // The memory saved a moment ago is missing.
        let missed = awards(case, vec![deploy, block(1, current[..2].to_vec())]);
        assert_eq!(award(&missed, "current_memories_injected"), Some(0));
    }

    #[test]
    fn a_trashed_bank_case_passes_only_on_no_memory_after_the_trash() {
        let case = case_named("memory_bank_trashed_mid_session");
        let channel = block(0, vec![line(case, 0)]);
        let gone = |blocks: Vec<MemoryBlock>, bank: Option<String>| {
            let mut seen = observed(&["a", "b"], blocks);
            seen.exchanges[1].memory_bank = bank;
            seen.rules_updates
                .push(update(1, "\nNo memory section applies now.\n"));
            evaluate(case, &evidence_of(case, seen))
        };
        let clean = gone(vec![channel.clone()], None);
        assert_eq!(clean.infrastructure_error, None);
        assert_eq!(award(&clean, "memory_gone_after_trash"), Some(50));
        assert_eq!(award(&clean, "channel_fact_injected"), Some(30));
        // The owner line on the next turn: the trash was ignored.
        let ignored = gone(
            vec![channel, block(1, vec![line(case, 1)])],
            Some(bank(RUN)),
        );
        assert_eq!(ignored.infrastructure_error, None);
        assert_eq!(award(&ignored, "memory_gone_after_trash"), Some(0));
    }

    /// The bank once memory's extraction read `session`: the live memories it
    /// learned from that session, and `rules`.
    fn read(session: &str, learned: &[&str], rules: &[(&str, &str)]) -> Extraction {
        Extraction {
            session_id: session.into(),
            waited_ms: 1200,
            memories: learned
                .iter()
                .enumerate()
                .map(|(index, text)| StoredMemory {
                    id: format!("fp{index}"),
                    text: text.to_string(),
                    session_id: Some(session.into()),
                    live: true,
                })
                .collect(),
            rules: rules
                .iter()
                .map(|(name, content)| (name.to_string(), content.to_string()))
                .collect(),
        }
    }

    /// What the teacher's extraction left in the bank.
    fn taught(learned: &[&str], rules: &[(&str, &str)]) -> Extraction {
        read("teach_a", learned, rules)
    }

    /// The case's evidence once its teacher left `teacher` in the bank.
    fn taught_evidence(
        case: &Case,
        answers: &[&str],
        blocks: Vec<MemoryBlock>,
        prompt: &str,
        teacher: Extraction,
    ) -> Evidence {
        let mut seen = observed(answers, blocks);
        let snapshot = Snapshot {
            teacher: Some(teacher),
            ..recorded(case, &mut seen, prompt, None)
        };
        assemble(case, RUN, seen, snapshot)
    }

    #[test]
    fn a_teacher_that_taught_nothing_fails_the_learned_gate() {
        let case = case_named("memory_learn_then_recall");
        let fact = render(case.teacher[0], RUN);
        let score = |teacher: Extraction| {
            let evidence = taught_evidence(case, &["x"], vec![], PROMPT, teacher);
            let evaluation = evaluate(case, &evidence);
            // Extraction ran and kept nothing: a score, never a rerun.
            assert_eq!(evaluation.infrastructure_error, None);
            award(&evaluation, "fact_learned")
        };
        assert_eq!(score(taught(&[], &[])), Some(0));
        assert_eq!(score(taught(&[&fact], &[])), Some(25));
        // Filed as an instruction, the fact is learned all the same.
        let rule = format!("# Learned\n- {fact}\n");
        assert_eq!(score(taught(&[], &[(LEARNED_RULE, &rule)])), Some(25));
        // Another session's memory, or one already retired, was not learned
        // from the teacher.
        let mut other = taught(&[&fact], &[]);
        other.memories[0].session_id = Some("e2e_a".into());
        assert_eq!(score(other), Some(0));
        let mut retired = taught(&[&fact], &[]);
        retired.memories[0].live = false;
        assert_eq!(score(retired), Some(0));
        // No read of the bank after the teacher: an infrastructure error.
        let unread = evaluate(case, &evidence_of(case, observed(&["x"], vec![])));
        assert_eq!(award(&unread, "fact_learned"), Some(0));
        let error = unread
            .infrastructure_error
            .expect("an unread teacher is flagged");
        assert!(error.contains("teacher"), "{error}");
    }

    #[test]
    fn a_learned_fact_reaches_the_turn_and_the_key_said_in_passing_stays_out() {
        let case = case_named("memory_learn_then_recall");
        let fact = render(case.teacher[0], RUN);
        let key = render(case.teacher[1], RUN);
        let command = format!("release-{} --channel stable", canary(RUN, "release"));
        let other = "This project never uses npm publish.";
        let run = |teacher: Extraction, lines: Vec<String>, prompt: &str, answer: &str| {
            let blocks = vec![block(0, lines)];
            evaluate(
                case,
                &taught_evidence(case, &[answer], blocks, prompt, teacher),
            )
        };
        let learned = || taught(&[&fact, other], &[]);
        // Anything memory learned from the chat may reach the turn.
        let clean = run(
            learned(),
            vec![fact.clone(), other.into()],
            PROMPT,
            &command,
        );
        assert_eq!(clean.infrastructure_error, None);
        for (id, weight) in [
            ("fact_learned", 25),
            ("learned_fact_reached_turn", 15),
            ("answer_uses_release_command", 30),
            ("key_never_stored", 15),
            ("key_never_repeated", 10),
            ("no_unrelated_memories", 5),
        ] {
            assert_eq!(award(&clean, id), Some(weight), "{id}");
        }
        let padded = run(
            learned(),
            vec![fact.clone(), DISTRACTORS[12].into()],
            PROMPT,
            &command,
        );
        assert_eq!(award(&padded, "no_unrelated_memories"), Some(0));
        // Learned, but the later session never got it.
        let unreached = run(learned(), vec![], PROMPT, &command);
        assert_eq!(award(&unreached, "learned_fact_reached_turn"), Some(0));
        // Learned as a rule, it reaches the turn in the memory section.
        let rule = format!("# Learned\n- {fact}\n");
        let section = format!("{PROMPT}{HEADER}\n## learned\n{rule}");
        let as_rule = run(
            taught(&[], &[(LEARNED_RULE, &rule)]),
            vec![],
            &section,
            &command,
        );
        assert_eq!(award(&as_rule, "learned_fact_reached_turn"), Some(15));
        // The key kept anywhere in the bank, even retired at once, fails.
        let mut kept = taught(&[&fact, &key], &[]);
        kept.memories[1].live = false;
        let stored = run(kept, vec![fact.clone()], PROMPT, &command);
        assert_eq!(award(&stored, "key_never_stored"), Some(0));
        let as_line = format!("# Learned\n- {key}\n");
        let in_rule = run(
            taught(&[&fact], &[(LEARNED_RULE, &as_line)]),
            vec![fact.clone()],
            PROMPT,
            &command,
        );
        assert_eq!(award(&in_rule, "key_never_stored"), Some(0));
        // Redacted before it was stored, it is not the key.
        let redacted = "The staging deploy key is [REDACTED:github_token].";
        let safe = run(
            taught(&[&fact, redacted], &[]),
            vec![fact.clone()],
            PROMPT,
            &command,
        );
        assert_eq!(award(&safe, "key_never_stored"), Some(15));
        // The key injected, in the memory section, or answered, fails.
        let injected = run(learned(), vec![fact.clone(), key.clone()], PROMPT, &command);
        assert_eq!(award(&injected, "key_never_repeated"), Some(0));
        let in_section = format!("{PROMPT}{HEADER}{key}\n");
        let sectioned = run(learned(), vec![fact.clone()], &in_section, &command);
        assert_eq!(award(&sectioned, "key_never_repeated"), Some(0));
        let answered = format!("{command} {}", canary(RUN, "k1"));
        let told = run(learned(), vec![fact.clone()], PROMPT, &answered);
        assert_eq!(award(&told, "key_never_repeated"), Some(0));
    }

    #[test]
    fn a_learned_instruction_reaches_the_next_session_as_a_standing_rule() {
        let case = case_named("memory_learn_rule_then_obey");
        let tag = canary(RUN, "prefix");
        let instruction =
            format!("Start every commit message with the prefix [{tag}] followed by one space.");
        let rule = format!("# Learned\n- {instruction}\n");
        let section = format!(
            "{PROMPT}{HEADER}\nThe rules below are the user's standing instructions.\n\n## learned\n{rule}"
        );
        let (first, second) = (format!("[{tag}] Fix crash"), format!("[{tag}] Add toggle"));
        let as_rule = evaluate(
            case,
            &taught_evidence(
                case,
                &[&first, &second],
                vec![],
                &section,
                taught(&[], &[(LEARNED_RULE, &rule)]),
            ),
        );
        assert_eq!(as_rule.infrastructure_error, None);
        for (id, weight) in [
            ("instruction_learned", 30),
            ("learned_as_standing_rule", 20),
            ("every_message_has_prefix", 50),
        ] {
            assert_eq!(award(&as_rule, id), Some(weight), "{id}");
        }
        // Filed as a memory and recalled: learned, but no standing rule.
        let recalled = evaluate(
            case,
            &taught_evidence(
                case,
                &[&first, &second],
                vec![block(0, vec![instruction.clone()])],
                PROMPT,
                taught(&[&instruction], &[]),
            ),
        );
        assert_eq!(award(&recalled, "instruction_learned"), Some(30));
        assert_eq!(award(&recalled, "learned_as_standing_rule"), Some(0));
    }

    #[test]
    fn a_learned_correction_must_hide_and_retire_the_stale_fact() {
        let case = case_named("memory_learn_correction");
        let (old, new) = (line(case, 0), render(case.teacher[0], RUN));
        let planted = |live: bool| StoredMemory {
            id: "fp-old".into(),
            text: old.clone(),
            session_id: None,
            live,
        };
        let answer = format!("wh-{}", canary(RUN, "new"));
        let run = |learned: &str, old_live: bool, lines: Vec<String>| {
            let mut teacher = taught(&[learned], &[]);
            teacher.memories.push(planted(old_live));
            let blocks = vec![block(0, lines)];
            evaluate(
                case,
                &taught_evidence(case, &[&answer], blocks, PROMPT, teacher),
            )
        };
        let replaced = run(&new, false, vec![new.clone()]);
        assert_eq!(replaced.infrastructure_error, None);
        for (id, weight) in [
            ("correction_learned", 25),
            ("correction_reached_turn", 15),
            ("stale_fact_left_out", 30),
            ("stale_fact_retired", 15),
            ("answer_has_current_warehouse", 15),
        ] {
            assert_eq!(award(&replaced, id), Some(weight), "{id}");
        }
        // Learned and reached, but the stale line still live and injected.
        let stale = run(&new, true, vec![new.clone(), old.clone()]);
        assert_eq!(award(&stale, "correction_learned"), Some(25));
        assert_eq!(award(&stale, "correction_reached_turn"), Some(15));
        assert_eq!(award(&stale, "stale_fact_left_out"), Some(0));
        assert_eq!(award(&stale, "stale_fact_retired"), Some(0));
        // A learned line that names the old warehouse too is the change
        // memory learned, not the stale fact.
        let change = format!(
            "The reporting service moved from wh-{} to wh-{}.",
            canary(RUN, "old"),
            canary(RUN, "new")
        );
        let moved = run(&change, false, vec![change.clone()]);
        assert_eq!(award(&moved, "stale_fact_left_out"), Some(30));
        assert_eq!(award(&moved, "stale_fact_retired"), Some(15));
    }

    #[test]
    fn an_automation_session_that_taught_memory_anything_fails() {
        let case = case_named("memory_automation_not_learned");
        assert!(audits_bank_after(case));
        let after = |bank: Extraction| {
            let mut seen = observed(&["Noted."], vec![]);
            let snapshot = Snapshot {
                bank_after: Some(bank),
                ..recorded(case, &mut seen, PROMPT, None)
            };
            evaluate(case, &assemble(case, RUN, seen, snapshot))
        };
        let gate = "nothing_learned_from_automation";
        let quiet = after(read("e2e_a", &[], &[]));
        assert_eq!(quiet.infrastructure_error, None);
        assert_eq!(award(&quiet, gate), Some(100));
        let fact = "This project cuts a stable release with ship-release.";
        assert_eq!(award(&after(read("e2e_a", &[fact], &[])), gate), Some(0));
        // Written and retired at once is still a write.
        let mut retired = read("e2e_a", &[fact], &[]);
        retired.memories[0].live = false;
        assert_eq!(award(&after(retired), gate), Some(0));
        let rule = "# Learned\n- Never use npm publish.\n";
        let ruled = read("e2e_a", &[], &[(LEARNED_RULE, rule)]);
        assert_eq!(award(&after(ruled), gate), Some(0));
        // Never read after the session: an infrastructure error, not a pass.
        let unread = evaluate(case, &evidence_of(case, observed(&["Noted."], vec![])));
        assert_eq!(award(&unread, gate), Some(0));
        assert!(unread.infrastructure_error.is_some());
    }

    #[test]
    fn a_case_that_reads_extraction_measures_what_it_learned_and_how_long_it_took() {
        let case = case_named("memory_learn_then_recall");
        let fact = render(case.teacher[0], RUN);
        let rule = "# Learned\nintro\n- one\n- two\n";
        let evidence = taught_evidence(
            case,
            &["x"],
            vec![],
            PROMPT,
            taught(&[&fact], &[(LEARNED_RULE, rule)]),
        );
        assert_eq!(measured(&evidence, "memory_learned"), Some(3.0));
        assert_eq!(measured(&evidence, "memory_extraction_ms"), Some(1200.0));
        let unit = |id: &str| {
            evidence
                .measurements
                .iter()
                .find(|measurement| measurement.id == id)
                .map(|measurement| measurement.unit.clone())
        };
        assert_eq!(unit("memory_extraction_ms").as_deref(), Some("ms"));
        // A case that reads no extraction measures neither.
        let plain = evidence_of(
            case_named("memory_relevant_recall"),
            observed(&["x"], vec![]),
        );
        assert_eq!(measured(&plain, "memory_learned"), None);
        assert_eq!(measured(&plain, "memory_extraction_ms"), None);
    }
}
