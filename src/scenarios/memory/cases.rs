//! The memory cases: what each plants, what the session does, and how the
//! result is judged. Seeds and rules are templates: `{label}` becomes a
//! run-scoped 8-hex canary, so the subject cannot know a value unless memory
//! delivered it.

use crate::scenarios::stable_seed;

/// One memory to save, with the exchanges that need it.
#[derive(Debug)]
pub struct Seed {
    pub text: &'static str,
    /// Seeds that say the same thing share a group.
    pub group: &'static str,
    /// Exchanges (0-based) whose task needs this fact.
    pub needed_on: &'static [usize],
    /// Exchanges where the fact is on topic but not needed: allowed there,
    /// never required.
    pub related_on: &'static [usize],
}

/// One standing rule to store; it belongs in the system prompt.
#[derive(Debug)]
pub struct RuleSeed {
    pub name: &'static str,
    pub content: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Every `(exchange, group)` a seed needs is injected there or still visible.
    NeededInjected,
    /// No injected line outside its seed's `needed_on` and `related_on`;
    /// unknown lines count.
    NothingIrrelevant,
    /// No line whose group is already in the same block or a visible earlier block.
    NoDuplicates,
    /// No injected line on any exchange belongs to the seed group `group`.
    GroupOmitted { group: &'static str },
    /// Exchange 0's memory section holds every case rule, untruncated.
    RulesInPrompt,
    /// The answer holds the canary for the label `canary`; `None` = every exchange.
    AnswerContains {
        exchange: Option<usize>,
        canary: &'static str,
    },
    /// No answer holds the canary for the label `canary`.
    AnswerOmits { canary: &'static str },
}

#[derive(Debug)]
pub struct Criterion {
    pub id: &'static str,
    pub weight: u8,
    pub gate: bool,
    pub description: &'static str,
    pub label: &'static str,
    pub check: Check,
}

#[derive(Debug)]
pub struct Case {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub protects: &'static str,
    /// Plant nothing; score against the same seeds as withheld.
    pub baseline: bool,
    /// Also plant `DISTRACTORS`, saved first.
    pub distractors: bool,
    pub memories: &'static [Seed],
    pub rules: &'static [RuleSeed],
    pub prompt: &'static str,
    pub followups: &'static [&'static str],
    /// `harness::compact` runs before this exchange's send.
    pub compact_before: Option<usize>,
    pub criteria: &'static [Criterion],
}

const fn gate(
    id: &'static str,
    weight: u8,
    check: Check,
    label: &'static str,
    description: &'static str,
) -> Criterion {
    Criterion {
        id,
        weight,
        gate: true,
        description,
        label,
        check,
    }
}

const fn scored(
    id: &'static str,
    weight: u8,
    check: Check,
    label: &'static str,
    description: &'static str,
) -> Criterion {
    Criterion {
        gate: false,
        ..gate(id, weight, check, label, description)
    }
}

const fn seed(text: &'static str, group: &'static str, needed_on: &'static [usize]) -> Seed {
    Seed {
        text,
        group,
        needed_on,
        related_on: &[],
    }
}

/// Facts about the user and the workplace that no case's task needs.
pub const DISTRACTORS: [&str; 20] = [
    "The user drinks green tea in the afternoon.",
    "The user's dog is named Biscuit.",
    "The user prefers dark mode in every editor.",
    "The user lives in Lisbon.",
    "The user plays chess on weekends.",
    "The user is learning Japanese.",
    "The office coffee machine is on the third floor.",
    "The quarterly planning meeting happens in March.",
    "The design team draws mockups in Figma.",
    "The marketing site is built with Astro.",
    "The user's favorite band is Radiohead.",
    "The user runs five kilometers every morning.",
    "The legal team reviews every press release.",
    "The user is allergic to peanuts.",
    "The analytics dashboards are hosted on Metabase.",
    "The user's laptop is a ThinkPad X1.",
    "Team lunches are on Fridays.",
    "The support rotation changes every Monday.",
    "The user enjoys science fiction novels.",
    "The user's standing desk is by the window.",
];

const RELEASE_PROMPT: &str = "Write the exact shell command I should run to cut a stable release of this project. Reply with the command only.";
const RELEASE_MEMORIES: &[Seed] = &[seed(
    "This project cuts a stable release with the command release-{release} --channel stable and never uses npm publish.",
    "release",
    &[0],
)];
const RELEASE_CRITERIA: &[Criterion] = &[
    gate(
        "release_fact_injected",
        40,
        Check::NeededInjected,
        "Release fact injected on the turn",
        "Was the stored release convention injected on the turn that needs it?",
    ),
    gate(
        "answer_uses_release_command",
        60,
        Check::AnswerContains {
            exchange: Some(0),
            canary: "release",
        },
        "Answer uses the stored release command",
        "Does the answer carry the run's release command value?",
    ),
];

const COMMIT_PROMPT: &str = "Write a commit message for a change that fixes a null pointer crash in the login form. Reply with the commit message only.";
const COMMIT_FOLLOWUPS: &[&str] = &["Now write a commit message for a change that adds a dark mode toggle to the settings page. Reply with the commit message only."];
const COMMIT_RULES: &[RuleSeed] = &[RuleSeed {
    name: "commit-messages",
    content:
        "Start every commit message you write with the prefix [{prefix}] followed by one space.",
}];
const COMMIT_CRITERIA: &[Criterion] = &[
    gate(
        "rule_in_system_prompt",
        30,
        Check::RulesInPrompt,
        "Rule present in the system prompt",
        "Does the system prompt of the first turn hold the stored commit rule, untruncated?",
    ),
    gate(
        "every_message_has_prefix",
        70,
        Check::AnswerContains {
            exchange: None,
            canary: "prefix",
        },
        "Every commit message has the prefix",
        "Does every answer carry the run's commit prefix value?",
    ),
];

pub const CASES: [Case; 11] = [
    Case {
        id: "memory_relevant_recall",
        title: "Memory recalls what the task needs",
        summary: "A bank holds two project facts the task needs among 20 unrelated user facts, and one task turn asks for both.",
        protects: "Memory puts the stored facts a task needs in front of the agent and nothing else. If it fails, agents miss facts the user already gave them or get buried in unrelated ones.",
        baseline: false,
        distractors: true,
        memories: &[
            seed(
                "Deploy the billing-sync service with the command deploy-{deploy}.",
                "deploy",
                &[0],
            ),
            seed(
                "Billing-sync logs are posted to the chat channel ops-{channel}.",
                "channel",
                &[0],
            ),
        ],
        rules: &[],
        prompt: "I need to deploy the billing-sync service and tell the team where to watch its logs. Reply with the exact deploy command and the name of the log channel.",
        followups: &[],
        compact_before: None,
        criteria: &[
            gate(
                "needed_memories_injected",
                40,
                Check::NeededInjected,
                "Both needed facts injected on the turn",
                "Were both stored facts the task needs injected on the turn that needs them?",
            ),
            gate(
                "no_unrelated_memories",
                30,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one the turn needs, with none of the stored distractors injected?",
            ),
            scored(
                "answer_has_deploy_command",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "deploy",
                },
                "Answer has the deploy command",
                "Does the answer carry the run's deploy command value?",
            ),
            scored(
                "answer_has_log_channel",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "channel",
                },
                "Answer names the log channel",
                "Does the answer carry the run's log channel value?",
            ),
        ],
    },
    Case {
        id: "memory_small_talk",
        title: "No memory on small talk",
        summary: "A bank holds 20 unrelated user facts, and the session only sends \"hi\", \"thanks!\", \"ok\" and \"?\".",
        protects: "Memory stays quiet when a message needs no stored fact. If it fails, every greeting drags unrelated user facts into the context and costs tokens.",
        baseline: false,
        distractors: true,
        memories: &[],
        rules: &[],
        prompt: "hi",
        followups: &["thanks!", "ok", "?"],
        compact_before: None,
        criteria: &[gate(
            "no_memory_on_small_talk",
            100,
            Check::NothingIrrelevant,
            "No memory injected on small talk",
            "Did no memory reach the context on any of the small-talk turns?",
        )],
    },
    Case {
        id: "memory_first_person_off_topic",
        title: "First person is not a reason to recall",
        summary: "A bank holds 20 unrelated user facts, and the user asks for help with a failing test.",
        protects: "A request that says \"I\" or \"my\" does not pull every stored fact about the user. If it fails, coding turns carry personal trivia and the tokens it costs.",
        baseline: false,
        distractors: true,
        memories: &[],
        rules: &[],
        prompt: "Can you help me fix my failing test? add(2, 2) returns 5 instead of 4 in my calculator module.",
        followups: &[],
        compact_before: None,
        criteria: &[gate(
            "no_unrelated_user_memories",
            100,
            Check::NothingIrrelevant,
            "No unrelated user memory injected",
            "Was no stored user fact injected into a turn that does not need it?",
        )],
    },
    Case {
        id: "memory_paraphrase",
        title: "One fact, one line",
        summary: "A bank stores one migration command in three paraphrases, and the task asks for it once.",
        protects: "A fact stored in several paraphrases is injected once. If it fails, the context fills with the same fact repeated and the tokens it costs.",
        baseline: false,
        distractors: true,
        memories: &[
            seed(
                "The ledger service applies its database migrations with migrate-{migrate}.",
                "migrate",
                &[0],
            ),
            seed(
                "Ledger database migrations are run with the migrate-{migrate} command.",
                "migrate",
                &[0],
            ),
            seed(
                "To migrate the ledger database, run migrate-{migrate}.",
                "migrate",
                &[0],
            ),
        ],
        rules: &[],
        prompt: "How do I run the ledger database migrations? Reply with the exact command.",
        followups: &[],
        compact_before: None,
        criteria: &[
            gate(
                "migration_fact_injected",
                35,
                Check::NeededInjected,
                "Migration fact injected on the turn",
                "Was the stored migration fact injected on the turn that needs it?",
            ),
            gate(
                "migration_fact_once",
                35,
                Check::NoDuplicates,
                "Migration fact injected only once",
                "Was the migration fact injected as one line, without its paraphrases beside it?",
            ),
            scored(
                "no_unrelated_memories",
                15,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one the turn needs?",
            ),
            scored(
                "answer_has_migrate_command",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "migrate",
                },
                "Answer has the migrate command",
                "Does the answer carry the run's migrate command value?",
            ),
        ],
    },
    Case {
        id: "memory_session_repeat",
        title: "Do not repeat what is still in context",
        summary: "Four turns discuss the payments API rate limit, and the context is compacted before the last one.",
        protects: "A fact is not injected again while its earlier injection is still in the context, and it comes back after a compaction when a turn needs it. If it fails, long sessions pay for the same facts every turn or lose them after a compaction.",
        baseline: false,
        distractors: true,
        memories: &[
            seed(
                "The payments API allows 240 requests per minute, set in the file limits-{limits}.toml.",
                "limit",
                &[0, 1, 2, 3],
            ),
            // Every prompt names the rate limit, so the approver is on topic throughout.
            Seed {
                related_on: &[0, 3],
                ..seed(
                    "Raising the payments API rate limit needs approval from the on-call lead named lead-{lead}.",
                    "approver",
                    &[1, 2],
                )
            },
        ],
        rules: &[],
        prompt: "What is the payments API rate limit, and which file sets it?",
        followups: &[
            "Who has to approve raising the payments API rate limit?",
            "Thanks. Summarize the payments API rate limit decision so far in two sentences.",
            "Remind me: which file sets the payments API rate limit?",
        ],
        compact_before: Some(3),
        criteria: &[
            gate(
                "needed_facts_present",
                30,
                Check::NeededInjected,
                "Needed facts present on every turn",
                "Was each fact a turn needs injected on that turn or still in the context?",
            ),
            gate(
                "no_repeat_while_visible",
                30,
                Check::NoDuplicates,
                "No fact repeated while still visible",
                "Was no fact injected again while its earlier injection was still in the context?",
            ),
            gate(
                "no_unrelated_memories",
                20,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one its turn needs?",
            ),
            scored(
                "answer_after_compaction_has_file",
                20,
                Check::AnswerContains {
                    exchange: Some(3),
                    canary: "limits",
                },
                "Answer after compaction names the file",
                "Does the answer after the compaction carry the run's limits file value?",
            ),
        ],
    },
    Case {
        id: "memory_updated_fact",
        title: "The current fact, not the stale one",
        summary: "A bank holds an old analytics export schedule and its newer replacement, and the task asks for the current one.",
        protects: "When two memories disagree about one subject, only the newer is injected. If it fails, agents act on schedules and settings the user already changed.",
        baseline: false,
        distractors: true,
        memories: &[
            seed(
                "The analytics export runs every Tuesday at 09:00 and writes to the bucket export-{old}.",
                "export-old",
                &[],
            ),
            seed(
                "The analytics export now runs every Thursday at 18:00 and writes to the bucket export-{new}.",
                "export",
                &[0],
            ),
        ],
        rules: &[],
        prompt: "When does the analytics export run, and which bucket does it write to?",
        followups: &[],
        compact_before: None,
        criteria: &[
            gate(
                "current_fact_injected",
                35,
                Check::NeededInjected,
                "Current fact injected on the turn",
                "Was the newer export schedule injected on the turn that needs it?",
            ),
            gate(
                "stale_fact_left_out",
                35,
                Check::GroupOmitted {
                    group: "export-old",
                },
                "Stale fact left out of memory",
                "Was the older export schedule, which the newer one replaced, left out?",
            ),
            scored(
                "answer_has_current_bucket",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "new",
                },
                "Answer has the current bucket",
                "Does the answer carry the run's current bucket value?",
            ),
            scored(
                "answer_omits_stale_bucket",
                15,
                Check::AnswerOmits { canary: "old" },
                "Answer omits the stale bucket",
                "Does the answer leave out the run's stale bucket value?",
            ),
        ],
    },
    Case {
        id: "memory_value_project",
        title: "Memory carries project conventions",
        summary: "The release command of a project is stored in memory and cannot be guessed, and the task asks for it.",
        protects: "A convention only memory holds reaches the agent when its task needs it. If it fails, agents guess a project's conventions and get them wrong.",
        baseline: false,
        distractors: true,
        memories: RELEASE_MEMORIES,
        rules: &[],
        prompt: RELEASE_PROMPT,
        followups: &[],
        compact_before: None,
        criteria: RELEASE_CRITERIA,
    },
    Case {
        id: "memory_value_project_baseline",
        title: "Project convention without memory",
        summary: "The same release task on an empty bank, to show what the agent does without memory.",
        protects: "The same task without memory is the control for the project convention case. If it fails, that is expected: only the gap to the case with memory means anything.",
        baseline: true,
        distractors: false,
        memories: RELEASE_MEMORIES,
        rules: &[],
        prompt: RELEASE_PROMPT,
        followups: &[],
        compact_before: None,
        criteria: RELEASE_CRITERIA,
    },
    Case {
        id: "memory_value_preference",
        title: "Memory carries a standing preference",
        summary: "A commit message prefix is stored as a rule, and the session asks for two commit messages.",
        protects: "A stored rule reaches the system prompt untruncated and the agent follows it in every reply. If it fails, agents ignore the preferences users set as standing instructions.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: COMMIT_RULES,
        prompt: COMMIT_PROMPT,
        followups: COMMIT_FOLLOWUPS,
        compact_before: None,
        criteria: COMMIT_CRITERIA,
    },
    Case {
        id: "memory_value_preference_baseline",
        title: "Standing preference without memory",
        summary: "The same two commit messages on an empty bank, to show what the agent does without memory.",
        protects: "The same task without memory is the control for the standing preference case. If it fails, that is expected: only the gap to the case with memory means anything.",
        baseline: true,
        distractors: false,
        memories: &[],
        rules: COMMIT_RULES,
        prompt: COMMIT_PROMPT,
        followups: COMMIT_FOLLOWUPS,
        compact_before: None,
        criteria: COMMIT_CRITERIA,
    },
    Case {
        id: "memory_rule_fit",
        title: "Only the rule that applies is obeyed",
        summary: "Two rules are stored, one for SQL and one for haiku, and the task writes a SQL query.",
        protects: "Every rule reaches the system prompt intact, and the agent applies only the one that fits its task. If it fails, rules get cut off, ignored, or leak into replies they do not belong to.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[
            RuleSeed {
                name: "sql-style",
                content: "When you write a SQL query, write every SQL keyword in uppercase and end the query with the comment -- {sql}.",
            },
            RuleSeed {
                name: "haiku-signature",
                content: "When you write a haiku, sign it on its own last line with {haiku}.",
            },
        ],
        prompt: "Write a SQL query that counts the orders placed in the last 7 days from a table named orders. Reply with the query only.",
        followups: &[],
        compact_before: None,
        criteria: &[
            gate(
                "rules_in_system_prompt",
                30,
                Check::RulesInPrompt,
                "Both rules intact in the system prompt",
                "Does the system prompt of the first turn hold both stored rules, untruncated?",
            ),
            gate(
                "sql_rule_obeyed",
                40,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "sql",
                },
                "SQL rule obeyed in the answer",
                "Does the answer carry the run's SQL comment value the SQL rule asks for?",
            ),
            gate(
                "unrelated_rule_ignored",
                30,
                Check::AnswerOmits { canary: "haiku" },
                "Unrelated rule left out of the answer",
                "Does the answer leave out the run's haiku signature value, a rule that does not apply?",
            ),
        ],
    },
];

pub fn bank(run_id: &str) -> String {
    format!("e2e-{run_id}")
}

/// Eight bare hex characters, fixed by the run and the label.
pub fn canary(run_id: &str, label: &str) -> String {
    format!(
        "{:08x}",
        stable_seed(&format!("memory:{run_id}:{label}")) as u32
    )
}

/// The `{label}` placeholders in `text`.
fn placeholders(text: &str) -> impl Iterator<Item = &str> {
    text.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(label, _)| label))
}

/// `template` with every `{label}` replaced by its canary for the run.
pub fn render(template: &str, run_id: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some((head, tail)) = rest.split_once('{') {
        let Some((label, after)) = tail.split_once('}') else {
            break;
        };
        out.push_str(head);
        out.push_str(&canary(run_id, label));
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The memories `setup` saves, rendered for the run, in saving order: the
/// distractors first when the case has them. A baseline plants nothing.
pub fn planted_memories(case: &Case, run_id: &str) -> Vec<String> {
    if case.baseline {
        return Vec::new();
    }
    let distractors: &[&str] = if case.distractors { &DISTRACTORS } else { &[] };
    distractors
        .iter()
        .copied()
        .chain(case.memories.iter().map(|seed| seed.text))
        .map(|template| render(template, run_id))
        .collect()
}

/// Every label the case plants or checks for, sorted and deduplicated.
pub fn labels(case: &Case) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = case
        .memories
        .iter()
        .map(|seed| seed.text)
        .chain(case.rules.iter().map(|rule| rule.content))
        .flat_map(placeholders)
        .chain(
            case.criteria
                .iter()
                .filter_map(|criterion| match criterion.check {
                    Check::AnswerContains { canary, .. } | Check::AnswerOmits { canary } => {
                        Some(canary)
                    }
                    _ => None,
                }),
        )
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case_named(id: &str) -> &'static Case {
        CASES
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("no case {id}"))
    }

    #[test]
    fn every_case_states_its_card() {
        for case in &CASES {
            assert!(!case.title.trim().is_empty(), "{} has no title", case.id);
            assert!(
                case.summary.ends_with('.') && !case.summary.trim_end_matches('.').contains(". "),
                "{}: the summary is one sentence",
                case.id
            );
            assert!(
                case.protects.contains(". If it fails, "),
                "{}: protects must say what breaks when it fails",
                case.id
            );
            for criterion in case.criteria {
                let words = criterion.label.split_whitespace().count();
                assert!(
                    (3..=8).contains(&words),
                    "{}: label of {} has {words} words",
                    case.id,
                    criterion.id
                );
            }
        }
    }

    #[test]
    fn every_answer_check_names_a_canary_the_case_plants() {
        for case in &CASES {
            let planted = |label: &str| {
                let marker = format!("{{{label}}}");
                case.memories.iter().any(|seed| seed.text.contains(&marker))
                    || case.rules.iter().any(|rule| rule.content.contains(&marker))
            };
            for criterion in case.criteria {
                if let Check::AnswerContains { canary, .. } | Check::AnswerOmits { canary } =
                    criterion.check
                {
                    assert!(planted(canary), "{}: {canary} is never planted", case.id);
                }
            }
        }
    }

    #[test]
    fn labels_are_found_in_seeds_rules_and_checks() {
        assert_eq!(
            labels(case_named("memory_relevant_recall")),
            ["channel", "deploy"]
        );
        assert_eq!(labels(case_named("memory_updated_fact")), ["new", "old"]);
        assert_eq!(labels(case_named("memory_rule_fit")), ["haiku", "sql"]);
        assert!(labels(case_named("memory_small_talk")).is_empty());
    }

    #[test]
    fn the_baselines_repeat_their_twins_without_planting() {
        for (twin, baseline) in [
            ("memory_value_project", "memory_value_project_baseline"),
            (
                "memory_value_preference",
                "memory_value_preference_baseline",
            ),
        ] {
            let (twin, baseline) = (case_named(twin), case_named(baseline));
            assert!(!twin.baseline && baseline.baseline);
            assert_eq!(twin.prompt, baseline.prompt);
            assert_eq!(twin.followups, baseline.followups);
            assert_eq!(twin.memories.len(), baseline.memories.len());
            assert_eq!(twin.rules.len(), baseline.rules.len());
            assert_eq!(twin.criteria.len(), baseline.criteria.len());
        }
    }

    #[test]
    fn a_case_plants_its_distractors_first_and_a_baseline_nothing() {
        let planted = planted_memories(case_named("memory_relevant_recall"), "a");
        assert_eq!(planted.len(), DISTRACTORS.len() + 2);
        assert_eq!(planted[0], DISTRACTORS[0]);
        assert_eq!(
            planted[DISTRACTORS.len()],
            render(
                "Deploy the billing-sync service with the command deploy-{deploy}.",
                "a"
            )
        );
        assert!(planted_memories(case_named("memory_rule_fit"), "a").is_empty());
        assert!(planted_memories(case_named("memory_value_project_baseline"), "a").is_empty());
    }

    #[test]
    fn canaries_differ_by_label_and_the_bank_names_the_run() {
        assert_ne!(canary("a", "deploy"), canary("a", "channel"));
        assert_eq!(bank("abc"), "e2e-abc");
    }
}
