//! Exact values: an amount, a date, a port, an environment variable and a
//! command, each planted and asked for, and a port stated in the middle of a
//! message of about 15,000 characters that memory must learn. A label's
//! suffix gives its canary the shape of the value (see `cases::canary`). They
//! run only in the `memory-gain` suite.

use super::cases::{gate, scored, seed, Case, Check, Criterion};

/// What a planted exact value is scored on: injected on the turn that asks
/// for it, then answered whole. `value` is the value as its seed states it.
const fn exact(value: &'static str) -> [Criterion; 2] {
    [
        gate(
            "exact_value_injected",
            40,
            Check::NeededInjected { exchange: None },
            "Exact value injected on the turn",
            "Was the stored exact value injected on the turn that asks for it?",
        ),
        gate(
            "answer_has_exact_value",
            60,
            Check::AnswerHolds { exchange: 0, value },
            "Answer carries the exact value",
            "Does the answer carry the run's exact value, as stored?",
        ),
    ]
}

/// One seed among the distractors, one question, nothing else.
const PLANTED: Case = Case {
    id: "",
    title: "",
    summary: "",
    protects: "",
    baseline: false,
    distractors: true,
    memories: &[],
    rules: &[],
    prompt: "",
    followups: &[],
    compact_before: None,
    preview_first: &[],
    teacher: &[],
    teacher_reply: None,
    changes: &[],
    criteria: &[],
};

pub const AMOUNT: Case = Case {
    id: "memory_precise_amount",
    title: "Memory recalls an exact amount",
    summary: "A bank holds the staging cloud account's monthly budget cap in US dollars among 20 unrelated user facts, and the task asks for the amount.",
    protects: "Memory hands a stored amount to the turn that asks for it, digit for digit. If it fails, an agent answers a budget question with a guess or a rounded figure.",
    memories: &[seed(
        "The monthly budget cap for the staging cloud account is USD {budget_usd}.",
        "budget",
        &[0],
    )],
    prompt: "What is the monthly budget cap for the staging cloud account? Reply with the amount only.",
    criteria: &exact("{budget_usd}"),
    ..PLANTED
};

pub const DATE: Case = Case {
    id: "memory_precise_date",
    title: "Memory recalls an exact date",
    summary: "A bank holds the expiry date of the payments-api TLS certificate among 20 unrelated user facts, and the task asks for that date.",
    protects: "Memory hands a stored date to the turn that asks for it, unchanged. If it fails, an agent plans a certificate renewal around a date it made up.",
    memories: &[seed(
        "The payments-api TLS certificate expires on {cert_date}.",
        "cert",
        &[0],
    )],
    prompt: "On which date does the payments-api TLS certificate expire? Reply with the date only, as YYYY-MM-DD.",
    criteria: &exact("{cert_date}"),
    ..PLANTED
};

pub const PORT: Case = Case {
    id: "memory_precise_port",
    title: "Memory recalls an exact port",
    summary: "A bank holds the port the metrics gateway listens on among 20 unrelated user facts, and the task asks for the port.",
    protects: "Memory hands a stored port number to the turn that asks for it. If it fails, an agent points a client at a default port and the connection fails.",
    memories: &[seed(
        "The metrics gateway listens on port {gateway_num}.",
        "gateway",
        &[0],
    )],
    prompt: "Which port does the metrics gateway listen on? Reply with the port number only.",
    criteria: &exact("{gateway_num}"),
    ..PLANTED
};

pub const ENV_VAR: Case = Case {
    id: "memory_precise_env_var",
    title: "Memory recalls an exact environment variable",
    summary: "A bank holds the environment variable the ledger worker reads its shard from among 20 unrelated user facts, and the task asks for its name.",
    protects: "Memory hands a stored environment variable name to the turn that asks for it. If it fails, an agent sets a variable the worker never reads.",
    memories: &[seed(
        "The ledger worker reads its shard from the environment variable LEDGER_SHARD_{shard_num}.",
        "shard",
        &[0],
    )],
    prompt: "Which environment variable does the ledger worker read its shard from? Reply with the variable name only.",
    criteria: &exact("LEDGER_SHARD_{shard_num}"),
    ..PLANTED
};

pub const COMMAND: Case = Case {
    id: "memory_precise_command",
    title: "Memory recalls an exact command",
    summary: "A bank holds the command that restarts the staging search indexer among 20 unrelated user facts, and the task asks for the command.",
    protects: "Memory hands a stored command to the turn that asks for it, flags included. If it fails, an agent restarts the indexer without the drain and drops work in flight.",
    memories: &[seed(
        "Restart the staging search indexer with the command indexctl restart --env staging --shard {restart} --drain 15m.",
        "restart",
        &[0],
    )],
    prompt: "Which exact command restarts the staging search indexer? Reply with the command only.",
    criteria: &exact("indexctl restart --env staging --shard {restart} --drain 15m"),
    ..PLANTED
};

/// About 1,100 characters of one day's narrative: nothing to keep, and no
/// digit that a port could be confused with.
macro_rules! day_notes {
    () => {
        "Then the usual run of the day, none of which needs keeping. I read through the open pull \
        requests, left a few comments about naming and error messages, and approved the small ones \
        that only touched documentation. A flaky test in the checkout flow failed again on my machine \
        and passed on the build server, so I added a note to look at its timers later rather than \
        chase it now. I joined a short call about the sprint, where we moved a handful of tickets \
        between columns and agreed to revisit the estimates once the design review is done. Over \
        lunch I skimmed a long thread about renaming a few modules; it went in circles and ended \
        without a decision, as these threads tend to. In the afternoon I cleaned up stale branches, \
        answered a question from a teammate about the local build cache, and spent a while reading \
        the profiler output for a slow admin page without trying a fix yet. The coffee machine on our \
        floor was broken again, the meeting room booking tool logged everyone out twice, and I closed \
        far more browser tabs than I would like to admit before moving on to the next day. "
    };
}

/// The user's notes over two weeks, one message of about 15,000 characters: a
/// cache bucket at its start, a webhook port in its middle. Memory cuts a user
/// line past 12,000 characters to its first and last 6,000, and the port sits
/// in that cut.
const PRECISE_NOTES: &str = concat!(
    "Some notes for later about this project, mostly a long account of the last two weeks. One \
    thing at the start does hold for good: the build cache bucket is cache-{notes_head} and \
    nothing else writes to it. The rest is background, day by day, and none of it needs keeping. ",
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    "One more thing in all this does hold for good: the billing webhook receiver listens on port \
    {notes_num} and nothing else may bind it. Back to the days. ",
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    day_notes!(),
    "That is all for now. Reply to this message with exactly one word: noted",
);

pub const LONG_MESSAGE: Case = Case {
    id: "memory_precise_long_message",
    title: "Memory learns an exact value from a long message",
    summary: "A chat with the user sends one message of about 15,000 characters with a cache bucket near its start and a webhook port in its middle, and a later session asks for the port.",
    protects: "A port stated in the middle of a very long message is learned like one at its start. If it fails, a user's long notes lose everything past their first and last 6,000 characters before extraction reads them, and nothing says so.",
    distractors: false,
    prompt: "Which port does the billing webhook receiver listen on? Reply with the port number only.",
    teacher: &[PRECISE_NOTES],
    // A reply that restated the port would teach it from a short line.
    teacher_reply: Some("noted"),
    criteria: &[
        // The control: the start survives a long line's cut, so extraction ran.
        gate(
            "head_learned",
            30,
            Check::Learned {
                canary: "notes_head",
            },
            "Fact at the start learned",
            "Did memory's extraction keep the cache bucket stated at the start of the long message?",
        ),
        gate(
            "middle_learned",
            40,
            Check::Learned {
                canary: "notes_num",
            },
            "Fact in the middle learned",
            "Did memory's extraction keep the webhook port stated in the middle of the long message?",
        ),
        scored(
            "middle_reached_turn",
            15,
            Check::Reached {
                exchange: 0,
                canary: "notes_num",
                section_only: false,
            },
            "Middle fact reached the later session",
            "Did the webhook port reach the later session's turn, injected or in its memory section?",
        ),
        scored(
            "answer_has_exact_value",
            15,
            Check::AnswerContains {
                exchange: Some(0),
                canary: "notes_num",
            },
            "Answer carries the exact value",
            "Does the answer carry the run's webhook port?",
        ),
    ],
    ..PLANTED
};
