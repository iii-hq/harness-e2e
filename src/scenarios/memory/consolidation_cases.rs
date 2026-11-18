//! Banks only a consolidated bank answers well: a stale fact memory's own
//! injection cannot tell is stale, and one step restated under nine labels
//! that crowds a second fact out. Each case also plants a twin, the first
//! seed's text without its final period: memory saves both, consolidation's
//! deterministic tier merges them without a model. They run only in the
//! `memory-gain` suite.

use super::cases::{gate, scored, seed, Case, Check, Seed};
use super::precise_cases::PLANTED;

pub const STALE_FACT: Case = Case {
    id: "memory_consolidation_stale_fact",
    title: "A consolidated bank drops a stale fact",
    summary: "A bank holds an analytics export bucket, a twin of it and a later line naming the bucket it changed to without a word memory reads as a change, and the task asks for the bucket.",
    protects: "Consolidation retires a fact memory's own injection cannot tell is stale. If it fails, the session reads both buckets and the consolidated arm measures nothing memory alone does not do.",
    memories: &[
        // The stale line comes first: the semantic judge supersedes only an
        // older line with a newer one.
        seed(
            "The analytics export writes to the bucket export-{stale}.",
            "export-old",
            &[],
        ),
        seed(
            "The analytics export writes to the bucket export-{stale}",
            "export-old",
            &[],
        ),
        // No change word (now, instead, moved...): memory tells both lines.
        seed(
            "The analytics export bucket changed at the storage review: it writes to export-{current}.",
            "export",
            &[0],
        ),
    ],
    prompt: "Which bucket does the analytics export write to?",
    criteria: &[
        gate(
            "current_fact_injected",
            20,
            Check::NeededInjected { exchange: None },
            "Current fact injected on the turn",
            "Was the line naming the current bucket injected on the turn that asks for it?",
        ),
        gate(
            "stale_fact_left_out",
            20,
            Check::GroupOmitted {
                group: "export-old",
            },
            "Stale fact left out of memory",
            "Was the line naming the older bucket, and its twin, left out?",
        ),
        scored(
            "answer_has_current_bucket",
            30,
            Check::AnswerContains {
                exchange: Some(0),
                canary: "current",
            },
            "Answer has the current bucket",
            "Does the answer carry the run's current bucket value?",
        ),
        scored(
            "answer_omits_stale_bucket",
            30,
            Check::AnswerOmits {
                exchange: None,
                canary: "stale",
            },
            "Answer omits the stale bucket",
            "Does the answer leave out the run's stale bucket value?",
        ),
    ],
    ..PLANTED
};

/// The one step every restart line states, behind a label of its own.
macro_rules! restart_step {
    () => {
        "restart the staging search indexer from the host bastion-{host}"
    };
}

const fn restart(text: &'static str) -> Seed {
    seed(text, "restart", &[0])
}

pub const REWORDED_FACT: Case = Case {
    id: "memory_consolidation_reworded_fact",
    title: "A consolidated bank makes room for a second fact",
    summary: "A bank holds one indexer restart step under nine labels, a twin of the first and a nightly job to pause before any restart, and the task asks for every step and precondition.",
    protects: "Consolidation merges lines that restate one step, so the lines memory tells have room for the fact the repeats crowd out. If it fails, a session gets the same step nine times and never hears that the nightly job must be paused first.",
    memories: &[
        restart(concat!("Runbook: ", restart_step!(), ".")),
        restart(concat!("Runbook: ", restart_step!())),
        restart(concat!("Reminder: ", restart_step!(), ".")),
        restart(concat!("Policy: ", restart_step!(), ".")),
        restart(concat!("Checklist: ", restart_step!(), ".")),
        restart(concat!("Handbook: ", restart_step!(), ".")),
        restart(concat!("Guideline: ", restart_step!(), ".")),
        restart(concat!("Procedure: ", restart_step!(), ".")),
        restart(concat!("Instruction: ", restart_step!(), ".")),
        restart(concat!("Convention: ", restart_step!(), ".")),
        seed(
            "Pause the nightly job reindex-{job} before any indexer restart, or the drain never finishes.",
            "pause",
            &[0],
        ),
    ],
    prompt: "How do I restart the staging search indexer? List every step and precondition you know of.",
    criteria: &[
        gate(
            "needed_facts_injected",
            20,
            Check::NeededInjected { exchange: None },
            "Needed facts injected on the turn",
            "Were the restart step and the job to pause first both injected on the turn?",
        ),
        scored(
            "answer_has_restart_host",
            20,
            Check::AnswerContains {
                exchange: Some(0),
                canary: "host",
            },
            "Answer has the restart host",
            "Does the answer carry the run's restart host?",
        ),
        scored(
            "answer_has_precondition",
            60,
            Check::AnswerContains {
                exchange: Some(0),
                canary: "job",
            },
            "Answer has the job to pause",
            "Does the answer carry the run's nightly job to pause before the restart?",
        ),
    ],
    ..PLANTED
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The twin: memory's save fingerprint keeps punctuation, consolidation's
    /// normalization drops it, so the pass supersedes one line with no model.
    #[test]
    fn each_case_plants_a_twin_that_differs_by_its_final_period() {
        for case in [&STALE_FACT, &REWORDED_FACT] {
            let twins = case.memories.windows(2).any(|pair| {
                pair[0].text.strip_suffix('.') == Some(pair[1].text)
                    && pair[0].group == pair[1].group
            });
            assert!(twins, "{}", case.id);
        }
    }

    /// Memory supersedes on its own only when the newer line holds one of
    /// these words; the current bucket must not, or memory alone drops the
    /// stale one.
    #[test]
    fn the_current_bucket_holds_no_word_memory_reads_as_a_change() {
        const CHANGE_WORDS: [&str; 9] = [
            "now",
            "longer",
            "anymore",
            "moved",
            "switched",
            "instead",
            "rescheduled",
            "replaced",
            "migrated",
        ];
        let current = STALE_FACT.memories[2].text;
        assert!(current.contains("{current}"));
        let words: Vec<String> = current
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
            .collect();
        for word in CHANGE_WORDS {
            assert!(!words.iter().any(|w| w == word), "{word}");
        }
    }

    /// Nine lines, one step: the same body behind nine labels, and no digit
    /// but the host's, so the semantic judge's merge passes its number check.
    #[test]
    fn the_restart_lines_differ_only_in_their_label() {
        let lines: Vec<&str> = REWORDED_FACT
            .memories
            .iter()
            .filter(|seed| seed.group == "restart" && seed.text.ends_with('.'))
            .map(|seed| seed.text)
            .collect();
        assert_eq!(lines.len(), 9);
        let split: Vec<(&str, &str)> = lines
            .iter()
            .map(|line| line.split_once(' ').unwrap())
            .collect();
        let labels: BTreeSet<&str> = split.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels.len(), 9, "{labels:?}");
        assert!(split.iter().all(|(_, body)| *body == split[0].1));
        for line in lines {
            let unplaced = line.replace("{host}", "");
            assert!(!unplaced.contains(|c: char| c.is_ascii_digit()), "{line}");
        }
    }
}
