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
    /// Entity handles `memory::save` files the memory under; none by default.
    pub entities: &'static [&'static str],
}

/// A change to the attempt's bank between two exchanges. A seed is an index
/// into the case's `memories`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Sets a rule; empty content removes it.
    RuleSet {
        name: &'static str,
        content: &'static str,
    },
    /// Saves a seed setup left out.
    Save { seed: usize },
    /// Deletes a saved seed's memory.
    Delete { seed: usize },
    /// Retires `old`'s memory in favor of `new`'s, which must be live.
    Supersede { old: usize, new: usize },
    /// Rewrites `seed`'s memory to the text of `to`, a seed setup left out.
    Update { seed: usize, to: usize },
    /// Trashes the attempt's bank.
    DeleteBank,
}

impl Change {
    /// The memory function the change calls.
    pub fn function(self) -> &'static str {
        match self {
            Change::RuleSet { .. } => "memory::rule::set",
            Change::Save { .. } => "memory::save",
            Change::Delete { .. } => "memory::delete",
            Change::Supersede { .. } => "memory::supersede",
            Change::Update { .. } => "memory::update",
            Change::DeleteBank => "memory::bank::delete",
        }
    }
}

/// One standing rule to store; it belongs in the system prompt.
#[derive(Debug)]
pub struct RuleSeed {
    pub name: &'static str,
    pub content: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Every `(exchange, group)` a seed needs is injected there or still
    /// visible; `None` = every exchange.
    NeededInjected { exchange: Option<usize> },
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
    /// No answer holds the canary for the label `canary`; `None` = every exchange.
    AnswerOmits {
        exchange: Option<usize>,
        canary: &'static str,
    },
    /// Exchange `exchange` carries exactly one rules update, holding the
    /// canary for `holds` and none of those for `omits`.
    UpdateOn {
        exchange: usize,
        holds: Option<&'static str>,
        omits: &'static [&'static str],
    },
    /// Exchange `exchange`'s per-turn rules reminder, on the attempt's bank,
    /// holds the canary for `holds` and none of those for `omits` (no
    /// reminder holds none).
    ReminderOn {
        exchange: usize,
        holds: Option<&'static str>,
        omits: &'static [&'static str],
    },
    /// Every exchange's system prompt carries memory's section, equal to
    /// exchange 0's: a rules change arrives as an update, never as an edit.
    SectionFrozen,
    /// From `exchange` on memory reads no bank and injects no block, and
    /// `exchange` carries the update saying no memory section applies.
    BankGoneFrom { exchange: usize },
    /// A live memory extracted from the teacher, or the bank's `learned`
    /// rule, holds the canary for `canary` once the teacher's extraction ran.
    Learned { canary: &'static str },
    /// Exchange `exchange`'s memory section, or unless `section_only` a
    /// memory injected on that exchange, holds the canary for `canary`.
    Reached {
        exchange: usize,
        canary: &'static str,
        section_only: bool,
    },
    /// Whether a memory (live, or any with `include_superseded`) or a rule of
    /// the bank after the teacher's extraction holds the canary for `canary`
    /// is `want`.
    Stored {
        canary: &'static str,
        include_superseded: bool,
        want: bool,
    },
    /// No injected memory, memory section, rules update or answer holds the
    /// canary for `canary`.
    NowhereContains { canary: &'static str },
    /// No memory of the seed group `group` is live in the bank after the
    /// teacher's extraction.
    Retired { group: &'static str },
    /// Once memory's extraction read the measured session, the bank holds
    /// what setup planted and nothing else: memory learned nothing from it.
    BankUnchanged,
}

impl Check {
    /// The labels whose canaries the check reads.
    pub fn canaries(self) -> impl Iterator<Item = &'static str> {
        let (first, rest): (_, &[&str]) = match self {
            Check::AnswerContains { canary, .. }
            | Check::AnswerOmits { canary, .. }
            | Check::Learned { canary }
            | Check::Reached { canary, .. }
            | Check::Stored { canary, .. }
            | Check::NowhereContains { canary } => (Some(canary), &[]),
            Check::UpdateOn { holds, omits, .. } | Check::ReminderOn { holds, omits, .. } => {
                (holds, omits)
            }
            _ => (None, &[]),
        };
        first.into_iter().chain(rest.iter().copied())
    }
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
    /// Plant nothing; score against the same seeds as withheld, on the twin's
    /// answer criterion alone. A baseline that completes produced the planted
    /// value without memory, so its twin proves nothing: redesign it.
    pub baseline: bool,
    /// Also plant `DISTRACTORS`, saved first.
    pub distractors: bool,
    pub memories: &'static [Seed],
    pub rules: &'static [RuleSeed],
    pub prompt: &'static str,
    pub followups: &'static [&'static str],
    /// `harness::compact` runs before this exchange's send.
    pub compact_before: Option<usize>,
    /// `(exchange, seed)`: once setup planted the bank, `memory::preview` of
    /// that exchange's message injects that seed's memory first. A ranking
    /// the case is built on; setup fails when it no longer holds, so the case
    /// is redesigned instead of passing without testing its point.
    pub preview_first: &'static [(usize, usize)],
    /// Messages a session of the user's own kind sends, in order, before the
    /// measured session starts, for memory's extraction to learn from.
    /// Templates, like the seeds.
    pub teacher: &'static [&'static str],
    /// The one word every teacher message asks the teacher to reply with.
    /// Setup fails on any other reply, so what extraction reads from the
    /// teacher's side is the same in every run: no reply restates a fact the
    /// case checks was learned from the user, or refuses to keep one.
    pub teacher_reply: Option<&'static str>,
    /// Changes to the bank, each run once exchange `.0` has its answer, in
    /// order and before any compaction.
    pub changes: &'static [(usize, Change)],
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
        entities: &[],
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
const fn release_answer(weight: u8) -> Criterion {
    gate(
        "answer_uses_release_command",
        weight,
        Check::AnswerContains {
            exchange: Some(0),
            canary: "release",
        },
        "Answer uses the stored release command",
        "Does the answer carry the run's release command value?",
    )
}
const RELEASE_CRITERIA: &[Criterion] = &[
    gate(
        "release_fact_injected",
        40,
        Check::NeededInjected { exchange: None },
        "Release fact injected on the turn",
        "Was the stored release convention injected on the turn that needs it?",
    ),
    release_answer(60),
];

const COMMIT_PROMPT: &str = "Write a commit message for a change that fixes a null pointer crash in the login form. Reply with the commit message only.";
const COMMIT_FOLLOWUPS: &[&str] = &["Now write a commit message for a change that adds a dark mode toggle to the settings page. Reply with the commit message only."];
const COMMIT_RULES: &[RuleSeed] = &[RuleSeed {
    name: "commit-messages",
    content:
        "Start every commit message you write with the prefix [{prefix}] followed by one space.",
}];
const fn prefix_answer(weight: u8) -> Criterion {
    gate(
        "every_message_has_prefix",
        weight,
        Check::AnswerContains {
            exchange: None,
            canary: "prefix",
        },
        "Every commit message has the prefix",
        "Does every answer carry the run's commit prefix value?",
    )
}
const COMMIT_CRITERIA: &[Criterion] = &[
    gate(
        "rule_in_system_prompt",
        30,
        Check::RulesInPrompt,
        "Rule present in the system prompt",
        "Does the system prompt of the first turn hold the stored commit rule, untruncated?",
    ),
    prefix_answer(70),
];

/// The user's own notes for a project, one message of about 4,400
/// characters: a release command at its start, a staging database host in
/// its middle, and the day's narrative around them, which holds nothing to
/// keep. Memory cuts a transcript line over 2,000 characters to its first and
/// last 1,000, so the host sits in the cut.
const LONG_NOTES: &str =
    "Some notes for later, all of these hold for this project: this project's release \
    command is release-{head} --channel stable, and nobody cuts a release from a laptop. \
    Before that, some background on where things stand today, none of which needs keeping. \
    This morning I spent a couple of hours chasing a flaky test in the checkout flow. It \
    failed about one run in five on my machine and never on the build server, which made it \
    slow to pin down. I first suspected the network mocks, then a race in the cart code, \
    and in the end it was a timer the test never waited for. I patched the test locally but \
    have not pushed the fix yet, so the branch is still red. After lunch I paired with a \
    colleague on the pricing page, mostly reading old pull requests to see why the discount \
    banner was hidden in some regions; we did not reach a conclusion and will look again \
    tomorrow. I also cleaned up a few stale branches, answered review comments on two small \
    pull requests, and started a short write-up of the flaky test for the team, which is \
    about half done. My laptop restarted twice for updates in the middle of all this, so a \
    couple of terminal sessions were lost and I had to set the local environment up again, \
    which took most of an hour. Around three I joined a call about the next sprint, where \
    we mostly moved tickets around and agreed to revisit the estimates later in the sprint. \
    Then I went back to the flaky test to make sure the patch held, ran the suite a dozen \
    times in a loop, and it stayed green every time, which is encouraging but not proof. \
    While the loop ran I skimmed the error dashboard: a spike of timeouts around noon that \
    cleared on its own, a handful of warnings from a library we are about to replace, and \
    nothing that looked new. I left a comment on the incident channel in case anyone else \
    saw the spike, but nobody had, so I let it go for today and moved on to the next thing. \
    I also helped a teammate whose local build kept failing; it was an old cache directory, \
    and clearing it fixed things in a few minutes, after half an hour of us looking \
    everywhere else. One thing in all this does hold for good: the staging database host is \
    db-{mid}.internal and only migrations touch it. Back to today: late in the afternoon I \
    looked at a slow page in the admin area that a customer complained about. The profiler \
    pointed at a query that loads every order for a customer when the page shows only ten, \
    but I ran out of time before trying a fix, so it is only a note in my scratch file for \
    now. I also read through a long thread about renaming a few modules, which went in \
    circles and ended without a decision. Someone asked whether we should move the nightly \
    jobs to an earlier slot; I said I would think about it and have not yet. The rest of \
    the day was small things: a typo in an error message, a broken link in the docs, a \
    dependency bump that needed a second look because its changelog was vague, and a short \
    chat with design about icon sizes on the settings screen, which they will follow up on. \
    I also spent twenty minutes on a test that turned out to be fine and just slow on a \
    cold cache. Tomorrow I will probably start with the slow admin page, then push the \
    flaky test fix, then see where the pricing question stands, though the plan may well \
    change by the morning stand-up. A few loose ends from the week, also just for context: \
    the demo earlier in the week went fine apart from a slow login that nobody could \
    reproduce afterwards, the new teammate finished their first small change and it is in \
    review, and the retrospective notes are still sitting in a draft that I promised to \
    tidy up. I tried a new terminal theme and switched back after an hour. The coffee \
    machine on our floor was broken again, which is not news. A vendor sent a long email \
    about pricing changes that I have not read properly; it can wait until next week. I \
    spent some time reading about a profiling tool a colleague recommended, but I have not \
    installed it, and I am not sure it fits how we work. There was also a short discussion \
    about whether to split the big integration test file, which ended with everyone \
    agreeing it is too long and nobody volunteering to do it this sprint. Finally, the \
    office plants got watered, the meeting room booking tool logged everyone out twice, and \
    I ended the day closing more browser tabs than I would like to admit. That is all for \
    now. Reply to this message with exactly one word: noted";

/// A release command, the control that extraction ran, then a lasting
/// diagnosis, which extraction keeps unless something forbids it (the control
/// case scores that it does). The diagnosis is only stated: the teacher sees
/// the rule, and asked to keep it, it refused instead of the fixed reply.
const DO_NOT_KEEP_TEACHER: &[&str] = &[
    "For later: this project cuts a stable release with the command release-{ctrl} --channel stable. Reply to this message with exactly one word: noted",
    "Also, I have been diagnosed with condition-{diag}, and my meals have to suit it. Reply to this message with exactly one word: noted",
];
/// It shares "diagnosed" and "condition" with the diagnosis, so a stored one
/// can reach the turn; the control scores whether it does.
const DO_NOT_KEEP_PROMPT: &str =
    "Suggest one dinner for me tonight that suits my diagnosed condition. Reply with the dish only.";

pub const CASES: [Case; 29] = [
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "needed_memories_injected",
                40,
                Check::NeededInjected { exchange: None },
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "migration_fact_injected",
                35,
                Check::NeededInjected { exchange: None },
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "needed_facts_present",
                30,
                Check::NeededInjected { exchange: None },
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "current_fact_injected",
                35,
                Check::NeededInjected { exchange: None },
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
                Check::AnswerOmits {
                    exchange: None,
                    canary: "old",
                },
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[release_answer(100)],
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[prefix_answer(100)],
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
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
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
                Check::AnswerOmits {
                    exchange: None,
                    canary: "haiku",
                },
                "Unrelated rule left out of the answer",
                "Does the answer leave out the run's haiku signature value, a rule that does not apply?",
            ),
        ],
    },
    Case {
        id: "memory_chatter_after_answer",
        title: "No memory on chatter after an answer",
        summary: "A bank holds a deploy command among 20 unrelated user facts, and the task is followed by two chatter turns, \"tell me more\" and \"I know\".",
        protects: "Short replies such as \"tell me more\" or \"I know.\" after an answer bring no stored facts about the user. If it fails, every bit of chatter pays for up to six unrelated user facts in the context.",
        baseline: false,
        distractors: true,
        memories: &[seed(
            "Deploy the billing-sync service with the command deploy-{deploy}.",
            "deploy",
            &[0],
        )],
        rules: &[],
        prompt: "Which command deploys the billing-sync service? Reply with the command only.",
        // Each phrase once read as a question about the user; whichever comes
        // first would inject, and the next finds the same facts still visible.
        followups: &["tell me more", "I know."],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "deploy_fact_injected",
                25,
                Check::NeededInjected { exchange: None },
                "Deploy fact injected on the task",
                "Was the stored deploy command injected on the task turn that needs it?",
            ),
            gate(
                "no_memory_on_chatter",
                60,
                Check::NothingIrrelevant,
                "No memory injected on chatter turns",
                "Did no stored fact reach the context on the chatter turns, and no unrelated one on the task turn?",
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
        ],
    },
    Case {
        id: "memory_user_fact_needed",
        title: "A task gets the user fact it needs",
        summary: "A bank holds the user's email closing among 20 unrelated user facts, and the task asks for an email that ends with it.",
        protects: "A task that asks for the user's own preference gets that stored fact about the user and no other. If it fails, a filter that keeps personal facts out of task turns also drops the preferences a task asks for.",
        baseline: false,
        distractors: true,
        memories: &[seed(
            "The user signs every email with the closing sign-{sign}.",
            "sign",
            &[0],
        )],
        rules: &[],
        prompt: "Write a two-line email saying the deploy is done, and end it with my usual email closing. Reply with the email only.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "user_fact_injected",
                40,
                Check::NeededInjected { exchange: None },
                "Needed user fact injected on the task",
                "Was the stored email closing injected on the turn that asks for it?",
            ),
            gate(
                "no_unrelated_memories",
                30,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory the one the task needs, with no other user fact beside it?",
            ),
            scored(
                "answer_has_closing",
                30,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "sign",
                },
                "Answer ends with the stored closing",
                "Does the answer carry the run's email closing value?",
            ),
        ],
    },
    Case {
        id: "memory_about_me",
        title: "What memory holds about the user",
        summary: "A bank holds six facts about the user and eight workplace facts, and the user asks what memory remembers about them.",
        protects: "Asking what memory holds about the user brings back the facts about the user and no workplace facts. If it fails, the usual way someone checks their memory comes back empty or padded with facts about their workplace.",
        baseline: false,
        // Its own list: six user facts, so memory's six-line cap never picks
        // among them, beside the eight workplace distractors.
        distractors: false,
        memories: &[
            seed("The user's team nickname is nick-{nick}.", "nick", &[0]),
            seed("The user's cat is named Pixel.", "cat", &[0]),
            seed("The user grew up in Porto.", "porto", &[0]),
            seed("The user plays the cello in a quartet.", "cello", &[0]),
            seed("The user's favorite author is Ursula Le Guin.", "author", &[0]),
            seed("The user is vegetarian.", "vegetarian", &[0]),
            seed(DISTRACTORS[6], "workplace", &[]),
            seed(DISTRACTORS[7], "workplace", &[]),
            seed(DISTRACTORS[8], "workplace", &[]),
            seed(DISTRACTORS[9], "workplace", &[]),
            seed(DISTRACTORS[12], "workplace", &[]),
            seed(DISTRACTORS[14], "workplace", &[]),
            seed(DISTRACTORS[16], "workplace", &[]),
            seed(DISTRACTORS[17], "workplace", &[]),
        ],
        rules: &[],
        prompt: "What do you remember about me?",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "user_facts_injected",
                40,
                Check::NeededInjected { exchange: None },
                "Every user fact injected",
                "Was every stored fact about the user injected on the turn that asks for them?",
            ),
            gate(
                "no_workplace_memories",
                30,
                Check::NothingIrrelevant,
                "No workplace memory injected",
                "Was no stored workplace fact injected on a turn about the user?",
            ),
            scored(
                "answer_has_nickname",
                30,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "nick",
                },
                "Answer names the stored nickname",
                "Does the answer carry the run's team nickname value?",
            ),
        ],
    },
    Case {
        id: "memory_reworded_across_turns",
        title: "A visible fact is not sent reworded",
        summary: "A bank holds one migration command in two wordings, and a second turn asks for it again while the first wording is still in context.",
        protects: "A fact the context already shows is not sent again in other words on a later turn. If it fails, long sessions carry the same fact twice under two wordings and pay for both.",
        baseline: false,
        distractors: true,
        memories: &[
            seed(
                "Ledger database migrations are run with the migrate-{migrate} command.",
                "migrate",
                &[0, 1],
            ),
            seed(
                "To migrate the ledger database, run migrate-{migrate}.",
                "migrate",
                &[0, 1],
            ),
        ],
        rules: &[],
        prompt: "How do I run the ledger database migrations? Reply with the exact command.",
        followups: &["Remind me how to migrate the ledger database."],
        compact_before: None,
        // The first wording is told first, and the follow-up ranks the second
        // above it: only then is the second sent unless memory sees the first.
        preview_first: &[(0, 0), (1, 1)],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "migration_fact_present",
                30,
                Check::NeededInjected { exchange: None },
                "Migration fact present on both turns",
                "Was the migration fact injected or still in the context on each turn?",
            ),
            gate(
                "no_rewording_while_visible",
                50,
                Check::NoDuplicates,
                "No rewording sent while still visible",
                "Was the migration fact never sent again in other words while its first wording was in the context?",
            ),
            scored(
                "answer_has_migrate_command",
                20,
                Check::AnswerContains {
                    exchange: None,
                    canary: "migrate",
                },
                "Every answer has the migrate command",
                "Does every answer carry the run's migrate command value?",
            ),
        ],
    },
    Case {
        id: "memory_sibling_facts",
        title: "Sibling facts both reach the turn",
        summary: "A bank holds two services on the same Postgres version among 20 unrelated user facts, and the task asks which services use it.",
        protects: "Two facts that share a value but name different subjects both reach a turn that asks about that value. If it fails, an upgrade plan for every service on Postgres 16 silently leaves one of them out.",
        baseline: false,
        distractors: true,
        // No canary: two hex values are never one fact, which would hide the
        // collapse; the checks read injection only, so nothing is guessable.
        memories: &[
            seed("The billing service uses Postgres 16.", "pg-billing", &[0]),
            seed("The ledger service uses Postgres 16.", "pg-ledger", &[0]),
        ],
        rules: &[],
        prompt: "Which of our services use Postgres 16? Reply with the service names.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "both_siblings_injected",
                70,
                Check::NeededInjected { exchange: None },
                "Both sibling facts injected",
                "Were both services on Postgres 16 injected, each as its own line?",
            ),
            scored(
                "no_unrelated_memories",
                30,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one the turn needs?",
            ),
        ],
    },
    Case {
        id: "memory_rule_changed_mid_session",
        title: "A rule changed mid-session arrives as an update",
        summary: "A commit prefix rule is edited after the first commit message and removed after the second, and the session asks for three commit messages.",
        protects: "A rule edited or removed during a chat reaches the next turn as one appended update, and the system prompt stays as it was. If it fails, the agent keeps applying a rule the user changed, or every change rewrites the prompt and throws away its cache.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[RuleSeed {
            name: "commit-messages",
            content: "Start every commit message you write with the prefix [{before}] followed by one space.",
        }],
        prompt: COMMIT_PROMPT,
        followups: &[
            COMMIT_FOLLOWUPS[0],
            "Now write a commit message for a change that bumps the lodash dependency to 4.17.21. Reply with the commit message only.",
        ],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[
            (
                0,
                Change::RuleSet {
                    name: "commit-messages",
                    content: "Start every commit message you write with the prefix [{after}] followed by one space.",
                },
            ),
            // Empty content removes the rule.
            (
                1,
                Change::RuleSet {
                    name: "commit-messages",
                    content: "",
                },
            ),
        ],
        criteria: &[
            gate(
                "edit_sent_as_update",
                20,
                Check::UpdateOn {
                    exchange: 1,
                    holds: Some("after"),
                    omits: &["before"],
                },
                "Edited rule sent as one update",
                "Did the edited rule reach the next turn as one update holding the new prefix and not the old one?",
            ),
            gate(
                "edit_reminded",
                10,
                Check::ReminderOn {
                    exchange: 1,
                    holds: Some("after"),
                    omits: &["before"],
                },
                "Reminder after the edit restates it",
                "Did the next turn's rules reminder hold the new prefix and not the old one?",
            ),
            gate(
                "removal_reminded",
                5,
                Check::ReminderOn {
                    exchange: 2,
                    holds: None,
                    omits: &["after", "before"],
                },
                "Reminder after the removal drops it",
                "Did the turn after the removal get no reminder holding either prefix?",
            ),
            gate(
                "removal_sent_as_update",
                15,
                Check::UpdateOn {
                    exchange: 2,
                    holds: None,
                    omits: &["after", "before"],
                },
                "Removed rule sent as an update",
                "Did the removal reach the next turn as one update without the removed prefix or the original one?",
            ),
            // shortcut: memory keeps the rules it told a session in-process, so a
            // memory restart mid-attempt re-renders the section and fails this
            // gate; tell a restart apart (memory's start time in the evidence)
            // once one shows up in a run.
            gate(
                "system_prompt_unchanged",
                20,
                Check::SectionFrozen,
                "System prompt section never rewritten",
                "Did every turn keep the memory section the first turn's system prompt froze?",
            ),
            // Scored: the frozen prompt still says the old prefix beside the update.
            scored(
                "edited_rule_obeyed",
                20,
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: "after",
                },
                "Edited rule obeyed on the next turn",
                "Does the answer after the edit carry the run's new prefix value?",
            ),
            scored(
                "removed_rule_not_applied",
                10,
                Check::AnswerOmits {
                    exchange: Some(2),
                    canary: "after",
                },
                "Removed rule no longer applied",
                "Does the answer after the removal leave out the removed prefix value?",
            ),
        ],
    },
    Case {
        id: "memory_bank_edits_mid_session",
        title: "Bank edits mid-session reach the next turn",
        summary: "After the first turn a memory is deleted, one replaced, one rewritten and one added, and the next turn asks about all four.",
        protects: "Memories deleted, replaced, edited or added while a chat runs reach the next turn as they now stand. If it fails, the agent acts on a fact the user removed or corrected, or misses one saved a moment ago.",
        baseline: false,
        distractors: true,
        // The first turn matches none of the retired texts, so none is still
        // visible on the next turn to pass for retired.
        memories: &[
            seed(
                "Deploy the billing-sync service with the command deploy-{deploy}.",
                "deploy",
                &[0],
            ),
            seed(
                "The orders service nightly backup writes to the bucket backup-{gone}.",
                "retired",
                &[],
            ),
            seed(
                "The orders API serves from the region region-{oldregion}.",
                "retired",
                &[],
            ),
            seed(
                "The orders service pages the on-call alias page-{oldalias}.",
                "retired",
                &[],
            ),
            seed(
                "The orders API serves from the region region-{newregion}.",
                "region",
                &[1],
            ),
            seed(
                "The orders service pages the on-call alias page-{newalias}.",
                "alias",
                &[1],
            ),
            seed(
                "The orders service status page is status-{added}.",
                "status",
                &[1],
            ),
        ],
        rules: &[],
        prompt: "Which command deploys billing-sync? Reply with the command only.",
        followups: &["For the orders service: which bucket does the nightly backup write to, which region serves the orders API, which alias pages the on-call engineer, and what is its status page? Reply with the four values."],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[
            (0, Change::Delete { seed: 1 }),
            (0, Change::Save { seed: 4 }),
            // No change word in the new text: only the tombstone retires the old.
            (0, Change::Supersede { old: 2, new: 4 }),
            (0, Change::Update { seed: 3, to: 5 }),
            (0, Change::Save { seed: 6 }),
        ],
        criteria: &[
            gate(
                "retired_memories_left_out",
                35,
                Check::GroupOmitted { group: "retired" },
                "Deleted, replaced and old text left out",
                "Were the deleted memory, the superseded one and the text an update replaced all left out?",
            ),
            gate(
                "current_memories_injected",
                35,
                Check::NeededInjected { exchange: None },
                "Current memories injected where needed",
                "Were the replacement, the rewritten text and the newly saved memory injected on the turn that asks for them?",
            ),
            scored(
                "no_unrelated_memories",
                10,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one its turn needs?",
            ),
            scored(
                "answer_has_new_region",
                10,
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: "newregion",
                },
                "Answer has the replacement region",
                "Does the answer carry the run's replacement region value?",
            ),
            scored(
                "answer_has_status_page",
                10,
                Check::AnswerContains {
                    exchange: Some(1),
                    canary: "added",
                },
                "Answer has the newly saved page",
                "Does the answer carry the run's status page value, saved after the first turn?",
            ),
        ],
    },
    Case {
        id: "memory_bank_trashed_mid_session",
        title: "A trashed bank gives the chat no memory",
        summary: "The attempt's bank is trashed after the first turn, and the next turn asks about a fact it held.",
        protects: "A bank trashed while a chat runs gives that chat no memory from the next turn on, never another bank's. If it fails, deleted facts and rules keep reaching the agent, or the chat falls back to the user's own main bank.",
        baseline: false,
        distractors: true,
        memories: &[
            seed(
                "Billing-sync logs are posted to the chat channel ops-{channel}.",
                "channel",
                &[0],
            ),
            // The line the next turn would get if the trash were ignored.
            seed(
                "The ledger service is owned by the team named owner-{owner}.",
                "owner",
                &[],
            ),
        ],
        rules: &[RuleSeed {
            name: "signature",
            content: "End every reply with a final line that reads sig-{sig}.",
        }],
        prompt: "Where are the billing-sync logs posted? Reply with the channel name.",
        followups: &["Which team owns the ledger service? Reply with the team name."],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[(0, Change::DeleteBank)],
        criteria: &[
            gate(
                "memory_gone_after_trash",
                50,
                Check::BankGoneFrom { exchange: 1 },
                "No memory after the bank is trashed",
                "Did the turn after the trash read no bank, get no memory, and get the note that no memory section applies?",
            ),
            gate(
                "channel_fact_injected",
                30,
                Check::NeededInjected { exchange: None },
                "Channel fact injected before the trash",
                "Was the stored log channel injected on the turn before the trash?",
            ),
            scored(
                "rule_applied_before_trash",
                10,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "sig",
                },
                "Signature rule applied before the trash",
                "Does the answer before the trash carry the run's signature value?",
            ),
            // Low weight: the frozen system prompt still holds the rule.
            scored(
                "rule_dropped_after_trash",
                10,
                Check::AnswerOmits {
                    exchange: Some(1),
                    canary: "sig",
                },
                "Signature rule dropped after the trash",
                "Does the answer after the trash leave out the run's signature value?",
            ),
        ],
    },
    Case {
        id: "memory_many_siblings",
        title: "Seven sibling facts all reach the turn",
        summary: "A bank holds seven services on the same Postgres version among 20 unrelated user facts, and the task asks which services use it.",
        protects: "Every stored fact a list question needs reaches the turn, not only as many as memory's per-turn line cap allows, while the context budget has room. If it fails, the agent answers which services use Postgres 16, or which repositories deploy one way, from a partial list and never says one is missing.",
        baseline: false,
        distractors: true,
        // No canary, as in the two-sibling case: two hex values are never one
        // fact, which would hide a collapse.
        memories: &[
            seed("The billing service uses Postgres 16.", "pg-billing", &[0]),
            seed("The ledger service uses Postgres 16.", "pg-ledger", &[0]),
            seed("The search service uses Postgres 16.", "pg-search", &[0]),
            seed("The mailer service uses Postgres 16.", "pg-mailer", &[0]),
            seed("The auth service uses Postgres 16.", "pg-auth", &[0]),
            seed("The export service uses Postgres 16.", "pg-export", &[0]),
            seed("The report service uses Postgres 16.", "pg-report", &[0]),
        ],
        rules: &[],
        prompt: "Which of our services use Postgres 16? Reply with the service names.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "all_siblings_injected",
                70,
                Check::NeededInjected { exchange: None },
                "All seven sibling facts injected",
                "Were all seven services on Postgres 16 injected, each as its own line?",
            ),
            scored(
                "no_unrelated_memories",
                30,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one the turn needs?",
            ),
        ],
    },
    Case {
        id: "memory_about_me_overflow",
        title: "Ten facts about the user all come back",
        summary: "A bank holds ten facts about the user and eight workplace facts, and the user asks what memory remembers about them.",
        protects: "Asking what memory holds about the user brings back every fact about the user, not the first six, and no workplace fact. If it fails, a user whose memory grew past six facts sees six of them and takes the rest for forgotten.",
        baseline: false,
        // Its own list: the six-fact case's facts and four more, past memory's
        // six-line cap, beside the same eight workplace distractors.
        distractors: false,
        memories: &[
            seed("The user's team nickname is nick-{nick}.", "nick", &[0]),
            seed("The user's cat is named Pixel.", "cat", &[0]),
            seed("The user grew up in Porto.", "porto", &[0]),
            seed("The user plays the cello in a quartet.", "cello", &[0]),
            seed("The user's favorite author is Ursula Le Guin.", "author", &[0]),
            seed("The user is vegetarian.", "vegetarian", &[0]),
            seed("The user's sister lives in Montreal.", "sister", &[0]),
            seed("The user speaks Portuguese and Italian.", "languages", &[0]),
            seed("The user was born in 1988.", "born", &[0]),
            seed("The user keeps bees on a rooftop.", "bees", &[0]),
            seed(DISTRACTORS[6], "workplace", &[]),
            seed(DISTRACTORS[7], "workplace", &[]),
            seed(DISTRACTORS[8], "workplace", &[]),
            seed(DISTRACTORS[9], "workplace", &[]),
            seed(DISTRACTORS[12], "workplace", &[]),
            seed(DISTRACTORS[14], "workplace", &[]),
            seed(DISTRACTORS[16], "workplace", &[]),
            seed(DISTRACTORS[17], "workplace", &[]),
        ],
        rules: &[],
        prompt: "What do you remember about me?",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "all_user_facts_injected",
                70,
                Check::NeededInjected { exchange: None },
                "All ten user facts injected",
                "Was every stored fact about the user, all ten, injected on the turn that asks for them?",
            ),
            gate(
                "no_workplace_memories",
                30,
                Check::GroupOmitted { group: "workplace" },
                "No workplace memory injected",
                "Was no stored workplace fact injected on a turn about the user?",
            ),
        ],
    },
    Case {
        id: "memory_linked_update",
        title: "A newer note about a person reaches the turn",
        summary: "A bank holds the payments-api on-call engineer, its escalation contact, another API's on-call engineer and a newer note that the payments-api engineer left, and the task asks whom to page.",
        protects: "A newer statement about someone the turn's facts name reaches that turn, even when it shares no word with the question. If it fails, the agent tells the user to page an engineer who left the company, because the departure was said about the person and never about the service.",
        baseline: false,
        // Twenty more memories, so meaning matches are weighed against a bank.
        distractors: true,
        memories: &[
            // On topic, so allowed; the departure makes it stale.
            Seed {
                related_on: &[0],
                entities: &["dana", "payments-api"],
                ..seed(
                    "dana-{oncall} is the primary on-call engineer for payments-api incidents.",
                    "oncall",
                    &[],
                )
            },
            Seed {
                entities: &["marco", "payments-api"],
                ..seed(
                    "Unanswered payments-api pages escalate to marco-{backup} after 15 minutes.",
                    "backup",
                    &[0],
                )
            },
            Seed {
                entities: &["kai", "ledger-api"],
                ..seed(
                    "kai-{other} is the primary on-call engineer for ledger-api incidents.",
                    "nearmiss",
                    &[],
                )
            },
            // Saved last, so newer than the on-call line; no word of the
            // question and no change word, only the entity both carry.
            // shortcut: only lexical-only recall surely leaves it out without
            // the link; with MiniLM its meaning may rank it, so check that
            // `memory::preview` omits it before a MiniLM run counts as a pass.
            Seed {
                entities: &["dana"],
                ..seed(
                    "dana-{oncall} left our company last week and her accounts were deactivated.",
                    "departure",
                    &[0],
                )
            },
        ],
        rules: &[],
        prompt: "Who should I page for a payments-api incident right now? Reply with the name only.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "update_and_backup_injected",
                55,
                Check::NeededInjected {
                    exchange: Some(0),
                },
                "Departure and escalation contact injected",
                "Were the newer note that the on-call engineer left and the escalation contact both injected on the turn?",
            ),
            // Scored: recall ranks the near-miss, and the link never reaches it.
            scored(
                "near_miss_left_out",
                15,
                Check::GroupOmitted { group: "nearmiss" },
                "Other API's on-call engineer left out",
                "Was the on-call engineer of another API left out?",
            ),
            scored(
                "answer_omits_departed_engineer",
                15,
                Check::AnswerOmits {
                    exchange: Some(0),
                    canary: "oncall",
                },
                "Answer omits the departed engineer",
                "Does the answer leave out the run's departed on-call engineer?",
            ),
            scored(
                "answer_names_escalation_contact",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "backup",
                },
                "Answer names the escalation contact",
                "Does the answer carry the run's escalation contact value?",
            ),
        ],
    },
    Case {
        id: "memory_learn_then_recall",
        title: "Memory learns a convention from a chat",
        summary: "A chat with the user states the release command and, in passing, a deploy key, and a later session asks for the release command.",
        protects: "A durable fact the user states in one chat reaches a later session that needs it, and a key said in passing is never kept. If it fails, users repeat their project's conventions in every new chat, or a credential said once reaches every later session.",
        baseline: false,
        distractors: true,
        memories: &[],
        rules: &[],
        prompt: RELEASE_PROMPT,
        followups: &[],
        compact_before: None,
        preview_first: &[],
        // The project convention case's fact, said in chat. The key comes in a
        // turn of its own, so its extraction pass is apart from the fact's.
        teacher: &[
            "Remember for later: this project cuts a stable release with the command release-{release} --channel stable and never uses npm publish.",
            "Also, the staging deploy key is ghp_{k1}{k2}{k3}{k4}0a1b.",
        ],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "fact_learned",
                25,
                Check::Learned { canary: "release" },
                "Release command learned from the chat",
                "Did memory's extraction keep the release command the user stated in the chat?",
            ),
            gate(
                "learned_fact_reached_turn",
                15,
                Check::Reached {
                    exchange: 0,
                    canary: "release",
                    section_only: false,
                },
                "Learned fact reached the later session",
                "Did the learned release command reach the later session's turn, injected or in its memory section?",
            ),
            release_answer(30),
            gate(
                "key_never_stored",
                15,
                Check::Stored {
                    canary: "k1",
                    include_superseded: true,
                    want: false,
                },
                "Key said in chat never stored",
                "Did no memory, superseded ones included, and no rule keep the deploy key the user said in passing?",
            ),
            gate(
                "key_never_repeated",
                10,
                Check::NowhereContains { canary: "k1" },
                "Key never injected or answered",
                "Did the deploy key stay out of every injected memory, memory section, rules update and answer?",
            ),
            scored(
                "no_unrelated_memories",
                5,
                Check::NothingIrrelevant,
                "No unrelated memory injected",
                "Was every injected memory one memory learned from the chat?",
            ),
        ],
    },
    Case {
        id: "memory_learn_rule_then_obey",
        title: "Memory learns a standing rule from a chat",
        summary: "A chat with the user asks for a commit message prefix from now on, and a later session asks for two commit messages.",
        protects: "An instruction the user gives in one chat becomes a standing rule that every later session follows. If it fails, users correct the agent the same way in every new chat.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[],
        prompt: COMMIT_PROMPT,
        followups: COMMIT_FOLLOWUPS,
        compact_before: None,
        preview_first: &[],
        teacher: &["From now on, start every commit message you write with the prefix [{prefix}] followed by one space."],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "instruction_learned",
                30,
                Check::Learned { canary: "prefix" },
                "Commit prefix instruction learned from chat",
                "Did memory's extraction keep the commit prefix instruction the user gave in the chat?",
            ),
            // Scored: filed as a memory instead, recall still brings it to the turn.
            scored(
                "learned_as_standing_rule",
                20,
                Check::Reached {
                    exchange: 0,
                    canary: "prefix",
                    section_only: true,
                },
                "Learned as a rule in the prompt",
                "Did the learned instruction reach the later session's system prompt as a standing rule?",
            ),
            prefix_answer(50),
        ],
    },
    Case {
        id: "memory_learn_correction",
        title: "Memory learns a correction from a chat",
        summary: "A bank holds the warehouse a reporting service reads from, a chat with the user corrects it, and a later session asks for it.",
        protects: "When the user corrects a fact memory holds, later sessions get only the corrected value. If it fails, agents keep using the warehouse, address or setting the user already corrected.",
        baseline: false,
        distractors: true,
        memories: &[seed(
            "The reporting service reads from the warehouse wh-{old}.",
            "old",
            &[],
        )],
        rules: &[],
        prompt: "Which warehouse does the reporting service read from? Reply with the warehouse name only.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &["Quick correction for later: the reporting service moved and now reads from the warehouse wh-{new}."],
        teacher_reply: None,
        changes: &[],
        criteria: &[
            gate(
                "correction_learned",
                25,
                Check::Learned { canary: "new" },
                "Corrected warehouse learned from the chat",
                "Did memory's extraction keep the corrected warehouse the user stated in the chat?",
            ),
            gate(
                "correction_reached_turn",
                15,
                Check::Reached {
                    exchange: 0,
                    canary: "new",
                    section_only: false,
                },
                "Corrected fact reached the later session",
                "Did the corrected warehouse reach the later session's turn, injected or in its memory section?",
            ),
            // Scored: the old line is hidden only when the extraction model
            // says the new one replaces it, or words it as a change.
            scored(
                "stale_fact_left_out",
                30,
                Check::GroupOmitted { group: "old" },
                "Stale warehouse left out of memory",
                "Was the warehouse the user corrected left out of what memory injected?",
            ),
            scored(
                "stale_fact_retired",
                15,
                Check::Retired { group: "old" },
                "Stale fact retired in the bank",
                "Did memory retire the stored warehouse the correction replaced?",
            ),
            scored(
                "answer_has_current_warehouse",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "new",
                },
                "Answer has the current warehouse",
                "Does the answer carry the run's corrected warehouse value?",
            ),
        ],
    },
    Case {
        id: "memory_automation_not_learned",
        title: "Non-user sessions teach memory nothing",
        summary: "A test session (kind \"e2e\", which memory treats like \"automation\" and every other non-user kind) asks memory to remember a release command, and once memory's extraction has read it the bank holds nothing new.",
        protects: "Automation and test sessions never teach memory anything, even when they say \"remember\". If it fails, every scripted run writes its fixtures into the user's memory.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[],
        // Sent as written, so no canary: any write fails the gate.
        prompt: "Remember for later: this project cuts a stable release with the command ship-release --channel stable and never uses npm publish.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[],
        teacher_reply: None,
        changes: &[],
        criteria: &[gate(
            "nothing_learned_from_automation",
            100,
            Check::BankUnchanged,
            "Nothing learned from a non-user session",
            "Once memory's extraction read the session, did the bank hold no memory and no rule beyond what setup planted?",
        )],
    },
    Case {
        id: "memory_learn_from_long_message",
        title: "Memory learns the middle of a long message",
        summary: "A chat with the user sends one message of about 4,400 characters with a release command near its start and a staging database host in its middle, and a later session asks for the host.",
        protects: "A durable fact in the middle of a long message is learned like one at its start. If it fails, a user's own notes for a project lose everything past their first and last thousand characters before extraction reads them, and nothing says so.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[],
        prompt: "Which host is the staging database? Reply with the host only.",
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: &[LONG_NOTES],
        // A reply that restated the host would teach it from a short line.
        teacher_reply: Some("noted"),
        changes: &[],
        criteria: &[
            // The control: the start survives a long line's cut, so extraction ran.
            gate(
                "head_learned",
                30,
                Check::Learned { canary: "head" },
                "Fact at the start learned",
                "Did memory's extraction keep the release command stated at the start of the long message?",
            ),
            gate(
                "middle_learned",
                40,
                Check::Learned { canary: "mid" },
                "Fact in the middle learned",
                "Did memory's extraction keep the staging database host stated in the middle of the long message?",
            ),
            scored(
                "middle_reached_turn",
                15,
                Check::Reached {
                    exchange: 0,
                    canary: "mid",
                    section_only: false,
                },
                "Middle fact reached the later session",
                "Did the staging database host reach the later session's turn, injected or in its memory section?",
            ),
            scored(
                "answer_has_staging_host",
                15,
                Check::AnswerContains {
                    exchange: Some(0),
                    canary: "mid",
                },
                "Answer has the staging host",
                "Does the answer carry the run's staging database host value?",
            ),
        ],
    },
    Case {
        id: "memory_honors_do_not_keep",
        title: "A rule not to remember binds memory",
        summary: "A bank holds the rule never to remember anything about the user's health, a chat with the user states a release command and a diagnosis, and a later session asks for a dinner that suits it.",
        protects: "A standing rule against remembering something binds what memory's extraction keeps, not only what the agent says. If it fails, a user who told memory to keep nothing about their health finds a diagnosis they mentioned once stored and handed to every later session.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[RuleSeed {
            name: "privacy",
            content: "Never remember anything about my health.",
        }],
        prompt: DO_NOT_KEEP_PROMPT,
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: DO_NOT_KEEP_TEACHER,
        // The teacher's prompt holds the rule: a reply refusing to keep the
        // diagnosis would change what extraction reads, unlike the control's.
        teacher_reply: Some("noted"),
        changes: &[],
        criteria: &[
            gate(
                "control_learned",
                20,
                Check::Learned { canary: "ctrl" },
                "Release command learned from the chat",
                "Did memory's extraction keep the release command, which shows it read the chat?",
            ),
            gate(
                "health_not_kept",
                50,
                Check::Stored {
                    canary: "diag",
                    include_superseded: true,
                    want: false,
                },
                "Diagnosis never stored",
                "Did no memory, superseded ones included, and no rule keep the diagnosis the rule forbids remembering?",
            ),
            gate(
                "health_never_repeated",
                30,
                Check::NowhereContains { canary: "diag" },
                "Diagnosis never injected or answered",
                "Did the diagnosis stay out of every injected memory, memory section, rules update and answer?",
            ),
        ],
    },
    Case {
        id: "memory_keeps_without_a_rule",
        title: "Without the rule the diagnosis is kept",
        summary: "The same chat on a bank without the rule, to show memory's extraction keeps the diagnosis when nothing forbids it.",
        protects: "The same chat without the rule is the control for the do-not-keep case: extraction must keep the diagnosis here, or that case passes without the rule doing anything. If it fails, the do-not-keep case proves nothing until its chat is reworded.",
        baseline: false,
        distractors: false,
        memories: &[],
        rules: &[],
        prompt: DO_NOT_KEEP_PROMPT,
        followups: &[],
        compact_before: None,
        preview_first: &[],
        teacher: DO_NOT_KEEP_TEACHER,
        teacher_reply: Some("noted"),
        changes: &[],
        criteria: &[
            gate(
                "control_learned",
                20,
                Check::Learned { canary: "ctrl" },
                "Release command learned from the chat",
                "Did memory's extraction keep the release command, which shows it read the chat?",
            ),
            gate(
                "health_kept",
                60,
                Check::Stored {
                    canary: "diag",
                    include_superseded: true,
                    want: true,
                },
                "Diagnosis stored without the rule",
                "Did a memory or a rule of the bank keep the diagnosis the user stated, with no rule against it?",
            ),
            // Scored: shows the measured prompt recalls a stored diagnosis, so
            // the do-not-keep case's injection gate is not vacuous.
            scored(
                "health_reached_turn",
                20,
                Check::Reached {
                    exchange: 0,
                    canary: "diag",
                    section_only: false,
                },
                "Diagnosis reached the later session",
                "Did the diagnosis reach the later session's turn, injected or in its memory section?",
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

/// The exchange after which a change saves seed `index`; `None` when setup
/// plants it.
pub fn saved_after(case: &Case, index: usize) -> Option<usize> {
    case.changes
        .iter()
        .find_map(|&(after, change)| match change {
            Change::Save { seed } | Change::Update { to: seed, .. } if seed == index => Some(after),
            _ => None,
        })
}

/// The exchange after which the case trashes its bank.
pub fn trashed_after(case: &Case) -> Option<usize> {
    case.changes
        .iter()
        .find_map(|&(after, change)| (change == Change::DeleteBank).then_some(after))
}

/// What `setup` saves, in saving order: the distractors first when the case
/// has them, then each seed no change saves later, with its index. A
/// baseline plants nothing.
pub fn planted(case: &Case) -> Vec<(Option<usize>, &'static str)> {
    if case.baseline {
        return Vec::new();
    }
    let distractors: &[&str] = if case.distractors { &DISTRACTORS } else { &[] };
    let seeds = case
        .memories
        .iter()
        .enumerate()
        .filter(|&(index, _)| saved_after(case, index).is_none())
        .map(|(index, seed)| (Some(index), seed.text));
    distractors
        .iter()
        .map(|&text| (None, text))
        .chain(seeds)
        .collect()
}

/// The memories `setup` saves, rendered for the run, in saving order.
pub fn planted_memories(case: &Case, run_id: &str) -> Vec<String> {
    planted(case)
        .into_iter()
        .map(|(_, template)| render(template, run_id))
        .collect()
}

/// Every template the case renders: its seeds, its rules, the rules its
/// changes set and its teacher's messages.
fn templates(case: &Case) -> impl Iterator<Item = &'static str> + '_ {
    let set = case.changes.iter().filter_map(|(_, change)| match change {
        Change::RuleSet { content, .. } => Some(*content),
        _ => None,
    });
    case.memories
        .iter()
        .map(|seed| seed.text)
        .chain(case.rules.iter().map(|rule| rule.content))
        .chain(set)
        .chain(case.teacher.iter().copied())
}

/// The labels whose canaries the case's teacher must make memory learn.
pub fn learned_labels(case: &Case) -> impl Iterator<Item = &'static str> + '_ {
    case.criteria
        .iter()
        .filter_map(|criterion| match criterion.check {
            Check::Learned { canary } => Some(canary),
            _ => None,
        })
}

/// Whether the case reads its bank once memory's extraction has read the
/// measured session.
pub fn audits_bank_after(case: &Case) -> bool {
    case.criteria
        .iter()
        .any(|criterion| criterion.check == Check::BankUnchanged)
}

/// Every label the case plants or checks for, sorted and deduplicated.
pub fn labels(case: &Case) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = templates(case)
        .flat_map(placeholders)
        .chain(
            case.criteria
                .iter()
                .flat_map(|criterion| criterion.check.canaries()),
        )
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// Runs every kind of change, for the tests of shapes no registered case uses yet.
#[cfg(test)]
pub(crate) const CHANGING: Case = Case {
    id: "memory_changes_fixture",
    title: "Every change between exchanges",
    summary: "A rule is edited, memories are deleted, saved, superseded and rewritten, and then the bank is trashed.",
    protects: "Nothing: it exercises the change shapes in unit tests. If it fails, a test is wrong.",
    baseline: false,
    distractors: false,
    memories: &[
        seed(
            "Deploy the billing-sync service with the command deploy-{deploy}.",
            "deploy",
            &[0],
        ),
        seed(
            "The orders service nightly backup writes to the bucket backup-{gone}.",
            "retired",
            &[],
        ),
        seed(
            "The orders API serves from the region region-{oldregion}.",
            "retired",
            &[],
        ),
        seed(
            "The orders service pages the on-call alias page-{oldalias}.",
            "retired",
            &[],
        ),
        seed(
            "The orders API serves from the region region-{newregion}.",
            "region",
            &[1],
        ),
        seed(
            "The orders service pages the on-call alias page-{newalias}.",
            "alias",
            &[1],
        ),
        Seed {
            entities: &["search"],
            ..seed("The search service runs on Elasticsearch 8.", "search", &[])
        },
    ],
    rules: &[RuleSeed {
        name: "commit-messages",
        content:
            "Start every commit message you write with the prefix [{before}] followed by one space.",
    }],
    prompt: "Which command deploys billing-sync?",
    followups: &[
        "Which region serves the orders API, and which alias pages its on-call engineer?",
        "Anything else?",
    ],
    compact_before: None,
    preview_first: &[],
    teacher: &[],
    teacher_reply: None,
    changes: &[
        (
            0,
            Change::RuleSet {
                name: "commit-messages",
                content: "Start every commit message you write with the prefix [{after}] followed by one space.",
            },
        ),
        (0, Change::Delete { seed: 1 }),
        (0, Change::Save { seed: 4 }),
        (0, Change::Supersede { old: 2, new: 4 }),
        (0, Change::Update { seed: 3, to: 5 }),
        (1, Change::DeleteBank),
    ],
    criteria: &[
        gate(
            "edit_sent_as_update",
            20,
            Check::UpdateOn {
                exchange: 1,
                holds: Some("after"),
                omits: &["before"],
            },
            "Edited rule sent as one update",
            "Did the edited rule reach the next turn as one update?",
        ),
        gate(
            "section_frozen",
            10,
            Check::SectionFrozen,
            "System prompt section never rewritten",
            "Did every turn keep the first turn's memory section?",
        ),
        gate(
            "current_injected_after_changes",
            20,
            Check::NeededInjected { exchange: Some(1) },
            "Current memories injected after the changes",
            "Were the memories saved and rewritten mid-session injected on the next turn?",
        ),
        gate(
            "retired_left_out",
            10,
            Check::GroupOmitted { group: "retired" },
            "Retired memories left out",
            "Were the deleted, superseded and rewritten texts left out?",
        ),
        scored(
            "old_prefix_dropped",
            10,
            Check::AnswerOmits {
                exchange: Some(1),
                canary: "before",
            },
            "Old prefix gone after the edit",
            "Does the answer after the edit leave out the old prefix?",
        ),
        gate(
            "memory_gone_after_trash",
            30,
            Check::BankGoneFrom { exchange: 2 },
            "No memory after the bank is trashed",
            "Did the turn after the trash get no memory and the note that none applies?",
        ),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn case_named(id: &str) -> &'static Case {
        CASES
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("no case {id}"))
    }

    /// Each `(seed, exchange)` where a seed a change saves after exchange `k`
    /// is needed or related on an exchange up to `k`, before it exists.
    fn needed_before_saved(case: &Case) -> Vec<(usize, usize)> {
        case.memories
            .iter()
            .enumerate()
            .flat_map(|(index, seed)| {
                let after = saved_after(case, index);
                seed.needed_on
                    .iter()
                    .chain(seed.related_on)
                    .filter(move |&&exchange| after.is_some_and(|after| exchange <= after))
                    .map(move |&exchange| (index, exchange))
            })
            .collect()
    }

    #[test]
    fn every_case_states_its_card() {
        for case in &CASES {
            assert!(!case.title.trim().is_empty(), "{} has no title", case.id);
            assert!(
                !case.baseline || (case.changes.is_empty() && case.teacher.is_empty()),
                "{}: a baseline plants nothing, so it changes and teaches nothing",
                case.id
            );
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

    /// Each index a case uses that its shape cannot honor.
    fn index_problems(case: &Case) -> Vec<String> {
        let mut problems = Vec::new();
        for (index, &(after, change)) in case.changes.iter().enumerate() {
            // `before_followup` runs only before a follow-up.
            if after >= case.followups.len() {
                problems.push(format!("a change after exchange {after} never runs"));
            }
            // A seed a later change saves does not exist yet: every attempt fails as infra.
            let named = match change {
                Change::Delete { seed } | Change::Update { seed, .. } => vec![seed],
                Change::Supersede { old, new } => vec![old, new],
                _ => Vec::new(),
            };
            for seed in named {
                let saved_later = case.changes[index + 1..]
                    .iter()
                    .any(|&(_, later)| match later {
                        Change::Save { seed: saved } | Change::Update { to: saved, .. } => {
                            saved == seed
                        }
                        _ => false,
                    });
                if saved_later {
                    problems.push(format!(
                        "change {index} names seed {seed} before it is saved"
                    ));
                }
            }
        }
        for criterion in case.criteria {
            match criterion.check {
                // Nothing needed there: the check would pass by construction.
                Check::NeededInjected { exchange: Some(k) }
                    if !case.memories.iter().any(|seed| seed.needed_on.contains(&k)) =>
                {
                    problems.push(format!("{}: nothing is needed on {k}", criterion.id));
                }
                Check::NeededInjected { exchange: None }
                    if !case.baseline
                        && case.memories.iter().all(|seed| seed.needed_on.is_empty()) =>
                {
                    problems.push(format!("{}: nothing is needed anywhere", criterion.id));
                }
                Check::GroupOmitted { group } | Check::Retired { group }
                    if !case.memories.iter().any(|seed| seed.group == group) =>
                {
                    problems.push(format!("{}: no seed is in group {group}", criterion.id));
                }
                // Memory sends an update only for a rules change or a trashed bank.
                Check::UpdateOn { exchange, .. }
                    if !case.changes.iter().any(|&(after, change)| {
                        after + 1 == exchange
                            && matches!(change, Change::RuleSet { .. } | Change::DeleteBank)
                    }) =>
                {
                    problems.push(format!(
                        "{}: no rules change runs before {exchange}",
                        criterion.id
                    ));
                }
                Check::BankGoneFrom { exchange }
                    if trashed_after(case).map(|after| after + 1) != Some(exchange) =>
                {
                    problems.push(format!(
                        "{}: the bank is not trashed before {exchange}",
                        criterion.id
                    ));
                }
                _ => {}
            }
        }
        for &(exchange, seed) in case.preview_first {
            if exchange > case.followups.len()
                || seed >= case.memories.len()
                || saved_after(case, seed).is_some()
            {
                problems.push(format!(
                    "preview of seed {seed} on {exchange}: not planted there"
                ));
            }
        }
        problems
    }

    #[test]
    fn every_change_check_and_preview_fits_the_cases_exchanges() {
        for case in CASES.iter().chain([&CHANGING]) {
            assert_eq!(index_problems(case), Vec::<String>::new(), "{}", case.id);
        }
        // Each rule can fail.
        let never_runs = Case {
            changes: &[(2, Change::Delete { seed: 1 }), (1, Change::DeleteBank)],
            criteria: &[],
            ..CHANGING
        };
        const NOTHING_NEEDED: &[Criterion] = &[gate(
            "x",
            100,
            Check::NeededInjected { exchange: Some(2) },
            "a b c",
            "d",
        )];
        const NOT_TRASHED: &[Criterion] = &[gate(
            "x",
            100,
            Check::BankGoneFrom { exchange: 1 },
            "a b c",
            "d",
        )];
        let nothing_needed = Case {
            criteria: NOTHING_NEEDED,
            ..CHANGING
        };
        let not_trashed = Case {
            criteria: NOT_TRASHED,
            ..CHANGING
        };
        let saved_later = Case {
            preview_first: &[(1, 4)],
            ..CHANGING
        };
        const NEEDED_ANYWHERE: &[Criterion] = &[gate(
            "x",
            100,
            Check::NeededInjected { exchange: None },
            "a b c",
            "d",
        )];
        let needed_nowhere = Case {
            memories: &[],
            criteria: NEEDED_ANYWHERE,
            ..CHANGING
        };
        const UNKNOWN_GROUP: &[Criterion] = &[gate(
            "x",
            100,
            Check::Retired { group: "typo" },
            "a b c",
            "d",
        )];
        let unknown_group = Case {
            criteria: UNKNOWN_GROUP,
            ..CHANGING
        };
        const UPDATE_ON_ONE: &[Criterion] = &[gate(
            "x",
            100,
            Check::UpdateOn {
                exchange: 1,
                holds: None,
                omits: &[],
            },
            "a b c",
            "d",
        )];
        let update_uncaused = Case {
            changes: &[(0, Change::Delete { seed: 1 })],
            criteria: UPDATE_ON_ONE,
            ..CHANGING
        };
        let named_before_saved = Case {
            changes: &[
                (0, Change::Delete { seed: 4 }),
                (0, Change::Save { seed: 4 }),
            ],
            criteria: &[],
            ..CHANGING
        };
        for case in [
            never_runs,
            nothing_needed,
            not_trashed,
            saved_later,
            needed_nowhere,
            unknown_group,
            update_uncaused,
            named_before_saved,
        ] {
            assert_eq!(
                index_problems(&case).len(),
                1,
                "{:?}",
                index_problems(&case)
            );
        }
    }

    /// Lowercase alphanumeric runs, as memory splits a message into words.
    fn words(text: &str) -> BTreeSet<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    }

    /// In lexical mode a follow-up sharing one word with a memory may still
    /// inject it, a limit memory documents; the chatter case tests the
    /// follow-up with no subject, so its words match nothing planted.
    #[test]
    fn the_chatter_follow_ups_share_no_word_with_anything_planted() {
        let case = case_named("memory_chatter_after_answer");
        assert!(case.distractors);
        let planted: BTreeSet<String> = DISTRACTORS
            .iter()
            .copied()
            .chain(case.memories.iter().map(|seed| seed.text))
            .flat_map(words)
            .collect();
        assert!(!case.followups.is_empty());
        for followup in case.followups {
            let shared: Vec<String> = words(followup).intersection(&planted).cloned().collect();
            assert!(shared.is_empty(), "{followup:?} shares {shared:?}");
        }
    }

    /// Prompts and follow-ups are sent as written: a `{label}` there would
    /// reach the subject with its braces.
    #[test]
    fn no_prompt_or_follow_up_holds_a_placeholder() {
        for case in &CASES {
            for message in std::iter::once(case.prompt).chain(case.followups.iter().copied()) {
                assert!(!message.contains('{'), "{}: {message:?}", case.id);
            }
        }
    }

    /// What a case checks memory learned, stored or kept out is what its
    /// teacher said: planted, the check would pass without any learning.
    #[test]
    fn a_learned_canary_is_taught_and_never_planted() {
        let mut taught = 0;
        for case in &CASES {
            for criterion in case.criteria {
                let (Check::Learned { canary }
                | Check::Stored { canary, .. }
                | Check::NowhereContains { canary }) = criterion.check
                else {
                    continue;
                };
                let marker = format!("{{{canary}}}");
                assert!(
                    case.teacher.iter().any(|message| message.contains(&marker)),
                    "{}: {canary} is never taught",
                    case.id
                );
                let mut planted = case
                    .memories
                    .iter()
                    .map(|seed| seed.text)
                    .chain(case.rules.iter().map(|rule| rule.content));
                assert!(
                    !planted.any(|template| template.contains(&marker)),
                    "{}: {canary} is planted",
                    case.id
                );
                taught += 1;
            }
        }
        assert_eq!(taught, 12);
    }

    /// Each case past the per-turn cap needs more lines on its one turn than
    /// memory's six, each a fact of its own.
    #[test]
    fn the_overflow_cases_need_more_lines_on_one_turn_than_the_six_line_cap() {
        for (id, needed) in [
            ("memory_many_siblings", 7),
            ("memory_about_me_overflow", 10),
        ] {
            let case = case_named(id);
            assert!(case.followups.is_empty(), "{id}");
            let groups: BTreeSet<&str> = case
                .memories
                .iter()
                .filter(|seed| !seed.needed_on.is_empty())
                .map(|seed| {
                    assert_eq!(seed.needed_on, [0], "{id}");
                    seed.group
                })
                .collect();
            assert_eq!(groups.len(), needed, "{id}");
        }
        // The two-sibling case, grown: each sibling leads with a service no
        // other names, as memory tells siblings apart by their first word.
        let siblings = case_named("memory_many_siblings");
        assert_eq!(siblings.prompt, case_named("memory_sibling_facts").prompt);
        let services: Vec<&str> = siblings
            .memories
            .iter()
            .map(|seed| seed.text.split(' ').nth(1).unwrap())
            .collect();
        for (index, seed) in siblings.memories.iter().enumerate() {
            let service = services[index];
            assert_eq!(
                seed.text,
                format!("The {service} service uses Postgres 16.")
            );
            let elsewhere = siblings
                .memories
                .iter()
                .filter(|other| words(other.text).contains(service));
            assert_eq!(elsewhere.count(), 1, "{service}");
        }
        // What the user asks about themselves, grown past the cap: every fact
        // of the six-fact case, and four more.
        let (overflow, about_me) = (
            case_named("memory_about_me_overflow"),
            case_named("memory_about_me"),
        );
        assert_eq!(overflow.prompt, about_me.prompt);
        for seed in about_me.memories {
            assert!(
                overflow
                    .memories
                    .iter()
                    .any(|other| other.text == seed.text && other.group == seed.group),
                "{}",
                seed.text
            );
        }
    }

    /// Memory keeps the first and last 1000 bytes of a transcript line over
    /// 2000: the control fact sits in the kept head, the middle fact wholly
    /// in the cut, more than 1000 characters from either end. Canaries render
    /// to eight characters, so one run stands for all.
    #[test]
    fn the_long_message_puts_its_middle_fact_where_memory_cuts_a_long_line() {
        let case = case_named("memory_learn_from_long_message");
        let [message] = case.teacher else {
            panic!("one teacher message")
        };
        let text = render(message, "a");
        assert!(text.is_ascii(), "bytes and characters must agree");
        assert!((4_000..5_000).contains(&text.len()), "{}", text.len());
        let span = |fact: &str| {
            let fact = render(fact, "a");
            let start = text.find(&fact).unwrap_or_else(|| panic!("{fact}"));
            assert_eq!(text.matches(&fact).count(), 1, "{fact}");
            (start, start + fact.len())
        };
        let (_, head_end) =
            span("this project's release command is release-{head} --channel stable");
        assert!(head_end < 1_000, "{head_end}");
        let (start, end) =
            span("the staging database host is db-{mid}.internal and only migrations touch it");
        assert!(start > 1_000 && end + 1_000 < text.len(), "{start}..{end}");
        assert_eq!(text.matches(&canary("a", "mid")).count(), 1);
    }

    /// Setup refuses a teacher reply other than the fixed one, so every
    /// teacher message asks for it, word for word.
    #[test]
    fn a_fixed_teacher_reply_is_asked_for_in_every_teacher_message() {
        let fixed: Vec<&Case> = CASES
            .iter()
            .filter(|case| case.teacher_reply.is_some())
            .collect();
        assert_eq!(fixed.len(), 3);
        for case in fixed {
            let ask = format!(
                "Reply to this message with exactly one word: {}",
                case.teacher_reply.unwrap()
            );
            assert!(!case.teacher.is_empty(), "{}", case.id);
            for message in case.teacher {
                assert!(message.ends_with(&ask), "{}: {message:?}", case.id);
            }
        }
    }

    /// The rule is all that tells the do-not-keep case from its control, a
    /// case of its own (a baseline never runs its teacher): the control shows
    /// extraction keeps the diagnosis, so the case cannot pass by construction.
    #[test]
    fn the_do_not_keep_case_and_its_control_differ_by_the_rule_alone() {
        let (case, control) = (
            case_named("memory_honors_do_not_keep"),
            case_named("memory_keeps_without_a_rule"),
        );
        assert!(!case.baseline && !control.baseline);
        assert_eq!(case.teacher, control.teacher);
        assert_eq!(case.teacher_reply, control.teacher_reply);
        assert!(case.teacher_reply.is_some());
        assert_eq!(case.prompt, control.prompt);
        assert_eq!(case.distractors, control.distractors);
        assert!(case.memories.is_empty() && control.memories.is_empty());
        assert_eq!(case.rules.len(), 1);
        assert!(control.rules.is_empty());
        let health = |case: &Case| {
            case.criteria
                .iter()
                .find_map(|criterion| match criterion.check {
                    Check::Stored {
                        canary: "diag",
                        include_superseded,
                        want,
                    } if criterion.gate => Some((include_superseded, want)),
                    _ => None,
                })
        };
        assert_eq!(health(case), Some((true, false)));
        assert_eq!(health(control), Some((true, true)));
    }

    /// The teacher's session sees the rule in its prompt: a diagnosis handed
    /// over to keep draws a refusal there, not the fixed reply, so the chat
    /// only states it.
    #[test]
    fn the_diagnosis_is_stated_without_asking_to_keep_it() {
        let case = case_named("memory_honors_do_not_keep");
        let ask = format!(
            "Reply to this message with exactly one word: {}",
            case.teacher_reply.unwrap()
        );
        let diagnosis = case
            .teacher
            .iter()
            .find(|message| message.contains("{diag}"))
            .expect("a teacher message states the diagnosis");
        let said = diagnosis.strip_suffix(&ask).unwrap().to_lowercase();
        for word in ["keep", "remember", "mind", "later", "store", "save"] {
            assert!(!said.contains(word), "{word:?} in {said:?}");
        }
    }

    /// The departure shares no word with the question and has no word memory
    /// reads as a change, so neither recall nor a supersede brings it to the
    /// turn: only the entity it shares with the on-call line links the two.
    #[test]
    fn the_linked_update_shares_only_an_entity_with_what_the_turn_gets() {
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
        let case = case_named("memory_linked_update");
        let seed = |group: &str| {
            case.memories
                .iter()
                .find(|seed| seed.group == group)
                .unwrap_or_else(|| panic!("{group}"))
        };
        let (oncall, departure) = (seed("oncall"), seed("departure"));
        let said = words(&render(departure.text, "a"))
            .into_iter()
            .chain(departure.entities.iter().flat_map(|entity| words(entity)))
            .collect::<BTreeSet<String>>();
        let asked = words(case.prompt);
        assert!(said.is_disjoint(&asked), "{:?}", said.intersection(&asked));
        assert!(CHANGE_WORDS.iter().all(|word| !said.contains(*word)));
        // Saved last: the newer statement about the person.
        assert_eq!(
            case.memories.last().map(|seed| seed.group),
            Some("departure")
        );
        assert!(oncall
            .entities
            .iter()
            .any(|entity| departure.entities.contains(entity)));
        // The near-miss is ranked by recall, not reached by the entity: scored.
        let near_miss = case
            .criteria
            .iter()
            .find(|criterion| criterion.check == Check::GroupOmitted { group: "nearmiss" })
            .unwrap();
        assert!(!near_miss.gate);
    }

    #[test]
    fn every_answer_check_names_a_canary_the_case_plants() {
        for case in CASES.iter().chain([&CHANGING]) {
            for canary in case.criteria.iter().flat_map(|c| c.check.canaries()) {
                let marker = format!("{{{canary}}}");
                assert!(
                    templates(case).any(|template| template.contains(&marker)),
                    "{}: {canary} is never planted",
                    case.id
                );
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
        // The key's labels are only in what the teacher says.
        assert_eq!(
            labels(case_named("memory_learn_then_recall")),
            ["k1", "k2", "k3", "k4", "release"]
        );
        assert!(labels(case_named("memory_automation_not_learned")).is_empty());
        // `after` is only in the rule a change sets.
        assert_eq!(
            labels(&CHANGING),
            [
                "after",
                "before",
                "deploy",
                "gone",
                "newalias",
                "newregion",
                "oldalias",
                "oldregion"
            ]
        );
    }

    /// The non-user case runs as the runner's own kind, and its card says so.
    #[test]
    fn the_non_user_case_names_the_kind_its_session_runs_as() {
        let case = case_named("memory_automation_not_learned");
        let kind = format!("\"{}\"", crate::wire::E2E_SESSION_KIND);
        assert!(case.summary.contains(&kind), "{}", case.summary);
    }

    #[test]
    fn a_seed_a_change_saves_is_not_planted_up_front() {
        assert_eq!(saved_after(&CHANGING, 4), Some(0));
        assert_eq!(saved_after(&CHANGING, 5), Some(0));
        assert_eq!(saved_after(&CHANGING, 3), None);
        assert_eq!(trashed_after(&CHANGING), Some(1));
        assert_eq!(trashed_after(case_named("memory_relevant_recall")), None);
        let seeds: Vec<Option<usize>> = planted(&CHANGING)
            .into_iter()
            .map(|(seed, _)| seed)
            .collect();
        assert_eq!(seeds, [Some(0), Some(1), Some(2), Some(3), Some(6)]);
        assert_eq!(
            planted_memories(&CHANGING, "a"),
            [0, 1, 2, 3, 6].map(|index| render(CHANGING.memories[index].text, "a"))
        );
    }

    #[test]
    fn a_seed_saved_after_exchange_k_is_needed_only_after_k() {
        for case in CASES.iter().chain([&CHANGING]) {
            let early = needed_before_saved(case);
            assert!(
                early.is_empty(),
                "{}: needed before saved: {early:?}",
                case.id
            );
        }
        // Saved after exchange 1, the region fact cannot serve exchange 1.
        let late = Case {
            changes: &[(1, Change::Save { seed: 4 })],
            ..CHANGING
        };
        assert_eq!(needed_before_saved(&late), [(4, 1)]);
    }

    /// A learning case reuses the baseline of the case that plants what it
    /// teaches: the same task, on an empty bank.
    #[test]
    fn the_baselines_repeat_their_twins_without_planting() {
        for (twin, baseline) in [
            ("memory_value_project", "memory_value_project_baseline"),
            (
                "memory_value_preference",
                "memory_value_preference_baseline",
            ),
            ("memory_learn_then_recall", "memory_value_project_baseline"),
            (
                "memory_learn_rule_then_obey",
                "memory_value_preference_baseline",
            ),
        ] {
            let (twin, baseline) = (case_named(twin), case_named(baseline));
            assert!(!twin.baseline && baseline.baseline);
            assert_eq!(twin.prompt, baseline.prompt);
            assert_eq!(twin.followups, baseline.followups);
            if twin.teacher.is_empty() {
                assert_eq!(twin.memories.len(), baseline.memories.len());
                assert_eq!(twin.rules.len(), baseline.rules.len());
            }
            // The answer alone: a baseline completes only when the subject
            // produced the planted value without memory.
            let [only] = baseline.criteria else {
                panic!("{}: one criterion", baseline.id)
            };
            assert!(only.gate && only.weight == 100, "{}", baseline.id);
            assert!(
                matches!(only.check, Check::AnswerContains { .. }),
                "{}",
                baseline.id
            );
            assert!(
                twin.criteria
                    .iter()
                    .any(|c| c.id == only.id && c.check == only.check),
                "{}: the twin's answer criterion",
                baseline.id
            );
            // The twin plants or teaches the value the baseline withholds.
            let marker = only.check.canaries().map(|label| format!("{{{label}}}"));
            for marker in marker {
                assert!(
                    templates(twin).any(|template| template.contains(&marker)),
                    "{}: {marker}",
                    twin.id
                );
            }
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
