# Harness E2E

`harness-e2e` measures what a Harness stack can execute: correct deliverables,
structural integrity, bounded work, and repeatable outcomes.

The repository stays independent of the `workers` source tree. Discovery,
execution, observation, state access, and cleanup go through functions
registered in iii. The product input is an already-running iii stack.

One binary, `harness-e2e`, does both jobs. Compose starts it as the
asynchronous `e2e::*` control plane and the injectable Console page. The same
binary also runs scenarios and inspects reports from the command line.

## Score

A run's score is the sum of the points its evaluated criteria awarded.

- A criterion nobody evaluated adds nothing.
- Scores are not normalized or rescaled.
- Criterion points stay independent of completion.
- A run stopped by a resource limit (token, cost, turn or time budget, or no
  progress), or one the agent or the Harness failed by itself, scores zero on
  every criterion it did not reach, in native, composite and adaptive flows
  alike: a counted run always has a score. A scenario that scores the partial
  delivery keeps those points.
- A check that holds only because nothing was attempted scores zero: a secret
  never fetched was not kept, an injection never read was not refused, a
  reply that wrote nothing was not quick, a source nobody worked on was not
  preserved.
- Criteria do not veto the score or approve a run.
- Completion, technical validity, artifact evidence, and runtime controls are
  reported separately.
- Each dimension report carries the raw points its criteria awarded. They add
  up to the score; they are a breakdown of it, not a second score.
- Infrastructure and execution failures still fail the CLI.

Native criteria keep awards that were already measured when a later check
cannot run. That later check has no award and stays `not_evaluated`. An
incomplete criterion set has no total score. A product that fails its checks
stays technically valid.

Every run records a `cause`, read from the failure's code and from the error
code the Harness recorded for the failed turn; a failure that kept no code
falls back to the runner's and the Harness's fixed wording. The cause decides
whether the run counts toward the mean. The subject's own failures
count: the run stays technically valid and incomplete, with the points already
measured. They are `resource_limit` (`max_turns` included, and a composite
flow's deadline or a Harness step's), `invalid_provider_request` (a request
the Harness built wrong, such as a tool call sent without its result or a
function schema the provider cannot read), `context_overflow` (also when the
provider only says so in its own words), `planner_format` and
`subject_failure`. The others leave the mean and are counted apart, per
scenario in `aggregate.causes`:

- `provider_infrastructure`: the provider was unreachable, rate limited, out
  of balance, credit or usage limit, or not authorized or configured
  (`provider_transport_failed` among them), with or without the turn's error
  entry.
- `test_environment_incomplete`: the final failure says the stack lacks a
  function or database the run is owed, or the stack does not route, single
  out or price the configured provider or model (an ambiguous model, no
  structured output, a cost cap without catalog pricing, a model the provider
  does not serve). The run is owed the functions it registers and needs
  itself (its worker contracts), the databases the prompt names and the
  functions of the measurement's own worker (`fp`) it names. A function the
  prompt alone mentions, such as a validator the scenario breaks on purpose,
  one the agent mistyped or made up, or one missing earlier in a turn that
  got past it, stays the subject's.
- `evaluation_environment`: the evaluator could not validate the delivery for
  a reason of its own machine or tooling, such as a Docker daemon that
  stopped answering. The Registry runner removes every other stack in the
  run's private Docker daemon before it starts the delivery, and records what
  it removed; a port still taken after that is the delivery's own. A check
  the delivery left unmeasurable (an alert the UI never shows, a comparison
  that never succeeds, a Compose file that does not load, a web container
  that does not restart) scores zero instead.
- `e2e_infrastructure`: any other failure of the runner or its fixtures, an
  engine the watchdog could not sample or a `harness::send` whose response
  the engine lost among them.
- `operator_cancelled`, also when the cancelled turn reports first.
- `undetermined`: another worker timed out, the turn was cancelled without an
  error, or the provider refused a request for a reason no rule recognises.

A function the subject registered that outlives its workers at cleanup is
recorded as the subject's `subject_leftover` failure; the run keeps its
status, cause and score.

Composite and adaptive flows follow the same rule: a failed composite step
keeps its turn's transcript, so its own error code decides, and a failed
planner turn is classified as a native turn. The Console counts failures by
cause.
A report written before causes keeps the earlier rule, where every subject
error left the mean, and every reader says so.

Every score and every audit flag is deterministic. No scenario calls a second
model to judge the first.

## Completion

Completed means the subject met the task's purpose. Every scenario marks the
criteria that are that purpose, usually one or two, as gates. Exactness,
discipline, efficiency, and polish never gate: they only move the score, so two
runs can both complete and still differ in points. A gate also checks that the
run exercised the capability the scenario protects, not only the final
artifact: a timer test completes only when the write came after the timer woke
the session, however clumsily the rest was done.

- Every gate passed: completed, whatever the other criteria scored. A gate
  passes at full points, or at its `gate_minimum` when fewer points already
  meet the purpose (a proportional check).
- A gate that did not pass: incomplete.
- A gate nobody evaluated: incomplete when another criterion already fell
  short, undetermined otherwise.

A scenario overrides this only when a prerequisite decides before any gate,
such as an application that never built. The gates are part of the behavior
digest, so changing one redefines the scenario.

Two Harness invariants hold on every native run that passed, whatever its
gates. They belong to no scenario's digest; the run's `harness_violations`
records what broke.

- A message whose calls the Harness dispatched (some have a result) leaves one
  without a result: incomplete, like a failed gate, and the score stays.
- A session on the subject model was fit into the context-manager's
  8192-token fallback limits while the catalog window is at least 32,768
  tokens: the router could not resolve the model, so nothing measured there
  can be trusted. It is an infrastructure error (`context_limits_unresolved`)
  the slot re-runs.

Outside the Harness mechanisms module, the scenarios that exercise a Harness
mechanism also gate the mechanism itself (`src/mechanism_gates/`): `context_pressure` its compactions, `contention_ledger`
its spawns, leaf wall and barrier, `timer_wake` its wake, `tool_contract_recovery`
its dispatch policy, `validation_self_repair` its validator binding,
`persistent_state` its invocation chokepoint, `shell_coder_sandbox` its
filesystem stamp, and a null-canary or onboarding run its agent profile. The
runner arms observers before the prompt and probes the stack before teardown;
the run's `mechanism_evidence` keeps what they saw. A probe the stack answers
with an error is judged like any other answer. A broken gate is a
`mechanism_gate` violation: incomplete, like a failed gate, and the score stays.
The `pre_generate` observer is a global hook (the Harness has no session filter
for it), so while one of these attempts runs, every step on the stack makes one
extra round trip to the runner. `persistent_state`'s observer also appends one
line to the prompt, which its behavior digest covers.

Each criterion has a `description`, the question the subject is scored on, and
a `label`, a 3–8 word statement the test card shows ("Stored record matches
the migration exactly"). The description is part of the behavior digest and
reaches the subject in some scenarios; the label does neither, so it can be
reworded without redefining anything. Kanban labels live in
`src/scenarios/kanban/labels.json`, beside the rubric, not in it.

## Requirements

`shell_coder_sandbox`, `chess_engine_build`, and `trend_blog` prepare their
reviewed fixture from an embedded Git bundle. They need Git. They do not need
a fixture checkout or `HARNESS_E2E_FIXTURE_PATH`. Each attempt uses a private
workspace, and temporary source checkouts are removed after their contents are
read.

`chess_engine_build` also needs a Compose daemon, a running iii Console, and an
interactive browser. It grades a run-scoped chess Worker through live functions,
the Console injectable-UI manifest, and the playable page at
`#/worker/<worker>/chess`, then preserves screenshots of the real Console with
the functional evidence. Setup installs the pinned iii SDK with npm before the
run; a cold npm cache needs Registry access.

`typescript_chat_service` carries its own frozen skeleton. It needs Node 22.6
or newer on the runner host: the subject's TypeScript application runs through
Node type stripping, both in the public suite and in the runner-owned
behavioral probe.

The trending-topics build needs Linux amd64, Docker, Git, Python 3, Node, and
access to its pinned fixture. See
[runtime and controls](tests/fixtures/trending-topics-build/README.md).

The engineering handoff uses a protected disposable checkout of a pinned
revision of `iii-hq/e2e-fixture`.

## Develop

From the repository root:

```bash
pnpm --dir dashboard install --frozen-lockfile
pnpm --dir dashboard typecheck
pnpm --dir dashboard lint
pnpm --dir dashboard test
pnpm --dir dashboard build
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
node --test tests/dashboard/*.test.cjs
HARNESS_E2E_BIN="$PWD/target/debug/harness-e2e" python3 -m unittest discover -s tests/python -p 'test_*.py'
```

The Rust build writes `dashboard/dist-console/page.js` and `styles.css`, then
embeds both in the worker. Node and pnpm must be on `PATH`.

List scenarios and their definition digests:

```bash
cargo run --locked --bin harness-e2e -- list
cargo run --locked --bin harness-e2e -- catalog
```

## Commands

| Command | What it does |
| --- | --- |
| *(none)* / `worker` | Run the Compose-managed control-plane service. |
| `list` | Print every scenario id. |
| `catalog` | Print the materialized scenario catalog. |
| `run` | Execute one or more scenarios against a running stack. |
| `report` | Summarize a saved `results.json`. |
| `models` | List models registered in the running stack. |
| `test-plan list` | List suites and their coverage. |
| `test-plan materialize` | Expand one suite into campaigns, groups, and cases. |
| `--manifest` | Print the Registry worker manifest as JSON. |

The E2E runner and launch scripts force `III_TELEMETRY_ENABLED=false` for
themselves and their child processes, preventing iii product usage events
from being sent to PostHog. For tests against an existing stack, set this
opt-out when starting its engine and Compose too. An engine already running
without the opt-out must be restarted with it before E2E tests.

Run one scenario against an existing stack:

```bash
cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex \
  --scenario todo_worker_simple
```

## Scenarios

Every scenario is a built-in module under `src/scenarios/`. The module owns
its prompt, setup, deterministic evaluator, and cleanup. Its id is the same
in the CLI, the worker catalog, the campaign runner, the dashboard, and the
canonical result artifacts. Plans name the scenarios they run.

Featured suites:

- [Registry](tests/fixtures/registry-version-comparison/README.md):
  `registry_planning`, `registry_implementation`, `registry_environment`, and
  `registry_verification`. Each has its own
  [atomic validations](tests/fixtures/registry-version-comparison/scoring.md).
- [Linkly tutorial](tests/fixtures/linkly-tutorial/README.md): the `linkly`
  suite. `linkly_tutorial` runs the seven tutorial chapters plus a
  project-restart guard as one scripted dialogue on one Harness session,
  against the `linkly-agentic` scaffold's Compose stack, and scores
  twenty-two deterministic checks. The `linkly` stack starts every group from
  a fresh scaffold; `scripts/linkly_stack.py` prepares one by hand.
- Onboarding tour: the `onboarding` suite. `onboarding_tour` takes the
  console tour of the onboarding worker the way a new user does, as one
  scripted dialogue under the `harness` template's Onboarding profile
  (`ade-worker-builder`), on the `onboarding` stack: the project
  `iii project init --learn-iii` creates. The test plays the tour page, closing each step as the page does,
  and checks what every step left behind.
- [Trending topics](docs/blog-build-contract.md): an isolated per-attempt Git
  remote and an independent Playwright acceptance check against the delivered
  SHA. Design is free. Screenshots are evidence, not an aesthetic score.

`config/test-plan.json` groups the catalog into modules (state, context,
wakes, coordination, software engineering, integration, security, adaptive
operations, continuous engineering, the Harness contract, the Harness
mechanisms, and the incremental Kanban application)
plus a diagnostic set that suites can opt into.

## Test plans

[config/test-plan.json](config/test-plan.json) is the executable plan. Rust
materializes its suites and execution rules from that source and from the
native scenario contracts. There is no generated catalog to keep in sync.

| Suite | ID | Purpose |
| --- | --- | --- |
| Core runtime regression | `regression` | Core runtime, recovery, context and safety checks, plus four probes of the Harness's own contract, eleven independent rounds (enough for the calibrated verdict to see a fall of 8 points); one technical retry where safe. Release Control runs it daily. |
| Null agent canary | `null-canary` | The regression's agent tasks run as the `null-agent` profile: every test must score 0 and stay incomplete. |
| Application builds and changes | `software-engineering` | Kanban, Registry delivery, the trending-topics blog, Alertmanager route migration and the chess, form-flow and state-machine Workers. |
| Application builds and changes ×3 (weekly) | `software-engineering-x3` | The same tests three independent times, to measure the suite's run-to-run spread. |
| Application builds and changes ×9 (harness comparison) | `software-engineering-x9` | All 14 cases nine independent times, on demand when a Harness release is judged, to compare two Harness versions. |
| Core runtime regression ×11 (harness comparison) | `regression-x11` | The regression's thirteen agent tasks eleven independent times, on demand when a Harness release is judged, to compare two Harness versions. |
| Context capacity (compaction) | `context-capacity` | Kanban chapters 1, 4 and 7 and context pressure, on the `context-capacity` stack. |
| Human approval of agent calls | `approvals` | One approved and one denied call through the approval gate, on the `approvals` stack. |
| Application builds with Deepwork | `deepwork` | The existing chess, form-flow and state-machine builds, on the `deepwork` stack with the `deepwork-e2e` profile. |
| Build Linkly from the tutorial | `linkly` | The Linkly agentic tutorial alone, on the `linkly` stack. |
| Complete the onboarding tour | `onboarding` | The onboarding tour a new user takes, alone, on the `onboarding` stack. |
| Memory quality | `memory` | What the memory worker injects into a live session, twenty-two cases three times each, on the `memory` stack. |
| Memory learning | `memory-learning` | What memory learns from a chat with the user and gives a later session, and that it learns nothing from a non-user (test or automation) session, seven cases three times each, on the `memory` stack. |
| Essential checks before merge | `pr` | Four essential checks of a candidate stack before merging a change. |
| Published stack checks | `after-release` | Five essential checks of the published stack. |

A suite is only what to test: scenarios, runs of each and technical retries.
In the Console these suites are read-only; **Suites** copies any of them into
a suite of the Console to edit, and **Run tests** runs a suite with a model on
the current Harness. See [suites](dashboard/README.md#suites).

```bash
cargo run --locked -- test-plan list
cargo run --locked -- test-plan materialize --suite software-engineering
```

The plan's digest, `definition_sha256`, covers the whole document, and every
suite's `profile_sha256` folds it in. Any edit to the plan, such as adding the
comparison and capacity suites or rewording a purpose, gives every suite a new
`profile_sha256`, even a suite whose tests did not change. Release Control
groups executions by `profile_sha256`, so the first release with the new plan
starts a new cohort for every suite. Compare across that release test by test,
by `behavior_sha256`, not suite by suite.

The `software-engineering` suite runs each selected case once, with no
technical retries: the seven Kanban cases, Registry implementation and verification,
the trending-topics build, Alertmanager route migration, and the chess,
form-flow and state-machine Workers in the Console. That is 14 cases and 14
planned runs, in 13 execution groups. Registry
implementation and verification share a group, in that order, so verification
receives the implementation delivery. Trending topics runs in
`case-trending-topics-build`. Baseline worker versions come from the resolved
stack contract. Alertmanager runs in its own group, needs Go 1.25+, and has no
turn or token ceiling.

A suite with `repetitions: 3` (`software-engineering-x3`) materializes three
campaigns, `<suite>-r01` to `-r03`, with the same groups and seeds; every group
of every round is its own job, so a round adds jobs, not time to a job.
`regression` runs eleven (`-r01` to `-r11`): 187 jobs, nine at a time.

Four of the regression's tests check a Harness mechanism the model has to set
in motion. `spawn_depth_cap` reads the Harness's `max_depth` from its
configuration entry and expects one session at each depth up to it and the
next spawn refused (`harness/spawn_depth_exceeded`, `spawn depth N+1 exceeds
max_depth N`); the executor stamps `max_depth: 1` on a stack that runs it, the
cheapest cap that still refuses, unless `depth_ladder` needs 6.
`repeated_failure_breaker` and `truncated_call_guard` check the two guards the
Harness answers locally: the third identical failing call and a call whose
arguments the output limit cut short never reach their run-scoped fixture. A
run where the model called the fixture but never provoked the guard (it
stopped retrying, or split the payload up front) is undetermined, with no
score; one that never called it is incomplete. `restart_recovery` restarts
session-manager between its first two exchanges and the Harness while its third
waits on a held call. Every regression test runs in a stack group, and so a
job, of its own, so the restarts reach no other test; it must never run on a
shared stack. It reads the session's status before the runner's
`harness::status`, which would repair a lost projection, and counts that read
only when the runner's wait ended on the turn's terminal event.

Four more of the regression's seventeen tests probe the Harness's own
contract rather than an agent's work: `harness_contract_probe` (the sends
`harness::send` must refuse, and the two output caps it must accept),
`dispatch_pipeline` (policy, hook chain, result normalization, hook holds and
discovery of a function call), `binding_contract_probe` (trigger bindings:
refusals, call bindings, wakes, the cap of 64 and teardown) and
`turn_failure_surface` (a turn refused by its budget preflight or a `pre_turn`
hook fails at zero tokens). The runner drives them directly, so each costs
less than a cent: one to six short turns, or none. Their hooks bind only the
run's own fixtures and sessions. `turn_failure_surface` declares
`expected_root_outcome = Failed`, and `harness_contract_probe` and
`binding_contract_probe` end with a bare follow-up, so the turns a probe
started are counted in the run's usage.

The `null-canary` suite runs the regression's thirteen agent tasks and `minimal_path`
(the one test `pr` and `after-release` add to the regression) once, with no
technical retries, and is meant to run as the `null-agent` Directory profile
([config/agents/null-agent.md](config/agents/null-agent.md)): it answers
`done` and calls no function. The four Harness contract probes stay out:
they score the Harness, which an agent that does nothing cannot fail. The
executor copies `config/agents/` into the
project's agents folder whenever an execution names a profile. Every run must
score 0 and stay `task_incomplete`; a point or a completion is a hole in that
test's evaluator, and a function call means the agent disobeyed, not that the
evaluator paid.

`cargo test` holds the same canary without a model, and its counterpart, for
every test a suite schedules. Each is scored as the runner scores it against a
null delivery (no call, one turn, a reply of `done` or one that only claims the
result), which must earn 0 and stay `task_incomplete`, and a reference
delivery, which must complete with 100; both run ten times and a result that
changes between rounds fails as drift.
The evaluators of the seven Kanban chapters and of the Registry,
trending-topics, chess, form-flow, state-machine, Linkly and onboarding tests
score what a live stack, Docker or a browser left behind. For each of them the
test names the reason and the deterministic controls CI runs instead
(`LIVE_CONTROLS` in `src/test_plan.rs`), and checks that CI runs every control
it names:

- Registry implementation and verification: `tests/python/test_registry_validation.py`
  scores the empty, the claim-only, the one-check-executed and the all-passing
  report, and
  `tests/python/test_registry_feature_probe.py` the feature probe's failure
  regressions.
- Trending topics: the evaluator, lifecycle and focus controls
  (`evaluator.test.mjs`, `test_lifecycle.py`,
  `tests/python/test_trending_focus_eval_regressions.py`). The reference app
  in `validate-controls.mjs` needs the private e2e-fixture app and runs by
  hand.
- Chess: the browser controls of `tests/fixtures/chess-eval/negative-control.mjs`,
  through `tests/python/test_chess_eval_regressions.py`.
- Form flow and state machine: the browser controls of
  `tests/fixtures/visual-worker-eval/regressions.mjs`, through
  `tests/python/test_visual_worker_eval_regressions.py`.
- Kanban, all seven chapters: the browser controls and scoring regressions in
  `tests/python/test_kanban_browser_flows.py` and
  `tests/python/test_kanban_scoring_regressions.py`. The base and reference
  controls of `scripts/kanban_eval/run.py` need Docker and the fixture and run
  by hand.
- Linkly and onboarding: no CI control.
- The four Harness contract probes: no CI control. Their subject is the
  Harness, not the agent; each scenario's unit tests score failing probe
  records offline.

The `linkly` suite runs the Linkly tutorial alone: its eight exchanges in
`case-linkly-tutorial`, once, with no technical retries. Run it on the `linkly`
stack, whose template starts every group from a fresh pinned `linkly-agentic`
scaffold as the Compose project that hosts the Harness; on another stack the
scenario's setup refuses the project it finds.

The `onboarding` suite runs the onboarding tour alone: its six prompts in
`case-onboarding-tour`, once, with no technical retries. Run it on the
`onboarding` stack, the `harness` template plus the tour's `onboarding`
container as `iii project init --learn-iii` seeds it; on another stack setup
refuses to start. The test always runs as the template's Onboarding profile,
`ade-worker-builder`: an execution that names no profile takes it, and one that
names another is refused. The test plays the `learn-iii` tour's card. Setup
closes the button steps (the sign-up box is skipped, never submitted) and binds
each other step's condition. When a condition fires, the step is closed with
`learn-iii::steps::complete`. After the dialogue the test adds two expenses
through the agent's expense tracker, one still to be reimbursed and one
already reimbursed, as the tracker's page would. It listens for the tracker's
own trigger after each and reads its totals before, between and after.
The objectives, gates in bold: **every tour step closed**, **the database
worker runs**, the Discoverability answer saying what each worker can do, **a new
worker with at least two functions**, its Console page, an expense's four
fields, a total still to be reimbursed that counts only the unreimbursed
expense, the tracker's own trigger type, **that trigger firing for each added
expense**, **a trigger the chat bound on it**, **the onboarding worker
removed**, and the tour served matching the fixture. Between prompts the test
waits, as the tour's card does, while the agent is still on a step: a turn
running or a wake it armed, up to twenty minutes.
[tests/fixtures/onboarding-tour/learn-iii.json](tests/fixtures/onboarding-tour/learn-iii.json)
is the tour the test was written against. If the worker serves a different
tour, the run loses the `tour.current` criterion and nothing else. When the
tour changes on purpose, regenerate the fixture from the worker's
`src/tours.mjs` with `scripts/onboarding_tour_fixture.mjs`.

The `memory` suite runs twenty-two cases, three independent rounds, one
technical retry. Each case plants its own bank (`e2e-<attempt>`) with the
rules, memories and unrelated distractor facts it calls for, waits until
memory has indexed all of them (with MiniLM; lexical-only stores nothing to
wait for), sends its prompt and follow-ups, and audits what memory injected
into each turn against what that turn needed: the right facts, no duplicates,
nothing unrelated, rules intact in the system prompt. A case may also change
its bank between turns (a rule set or removed, memories deleted, superseded,
rewritten or saved, the bank trashed) and judge what the next turn gets: a
rules change must arrive as one appended `<memory update="rules">` notice
while the system prompt keeps the section the first turn froze. The subject
may call no function (`memory::*` is denied), so a planted value reaches the
answer only through memory's injection. Two cases are baselines: the same task
with nothing planted, expected to fail, since a baseline that passes means its
value case proves nothing. Scoring is deterministic, never a model.

Beside recall, small talk, first person, paraphrases, repeats, stale facts,
rule fit and the two value cases with their baselines, eight cases cover:

- `memory_chatter_after_answer`: "tell me more" and "I know." after an answer
  inject nothing. Their words match nothing the case plants, since in
  lexical-only mode one shared word can still inject a memory.
- `memory_user_fact_needed`: a task that asks for the user's own preference
  gets that fact about the user, and no other.
- `memory_about_me`: "What do you remember about me?" brings back the six
  facts about the user and none of the eight workplace facts.
- `memory_reworded_across_turns`: a fact still in context is not sent again in
  other words. Setup checks with `memory::preview` that memory still ranks the
  two wordings as the case needs; when it does not, the attempt is an
  infrastructure error, so the case is redesigned instead of passing without
  testing its point.
- `memory_sibling_facts`: two services on Postgres 16 both reach the turn that
  asks which services use it.
- `memory_rule_changed_mid_session`: a rule edited, then removed, arrives as
  one update each time, the per-turn rules reminder follows it, and the
  system prompt never changes.
- `memory_bank_edits_mid_session`: a memory deleted, one superseded, one
  rewritten and one saved between turns reach the next turn as they now stand.
- `memory_bank_trashed_mid_session`: once the bank is trashed, the next turn
  reads no bank, gets no memory, and is told that no memory section applies.

Three more cases were written to fail before memory changes, to prove a gap
exists:

- `memory_many_siblings`: seven services on Postgres 16 all reach the turn that
  asks which use it. A memory that cuts a turn to six lines by count, before
  its token budget, always leaves one out.
- `memory_about_me_overflow`: "What do you remember about me?" over ten facts
  about the user brings back all ten and none of the eight workplace facts.
  The same six-line cut returns six.
- `memory_linked_update`: a newer note that the payments-api on-call engineer
  left reaches the turn that asks whom to page, beside the escalation
  contact. The note shares no word with the question and has no word memory
  reads as a change; only the entity it shares with the on-call line links
  the two. It is built for lexical-only retrieval, as on the Linux builds CI
  and the local recipe below use: with MiniLM, the note's meaning could rank
  it on its own.
  The other API's on-call engineer left out is scored, since recall ranks it
  and the link never reaches it.

`stacks/memory.yaml` runs the released `memory`, so the gap cases
(`memory_many_siblings`, `memory_about_me_overflow`, `memory_linked_update`,
`memory_learn_from_long_message`, `memory_honors_do_not_keep`) fail until a
memory release carries these fixes.

Each run also records `memory_` measurements: expected, irrelevant and
duplicate injections, the injected, rules and reminder tokens, the turns where
the memory hook did not run (a turn after the case trashed its bank is not
one), and, for a case that changes its rules or trashes its bank, the rules
updates memory sent (`memory_rules_updates`). An attempt that cannot show
memory ran on its own bank is an infrastructure error, reported unscored and
not retried automatically, so rerun the case: no hook, another bank, a recall
count its parsed lines do not match, a rules update memory announced that did
not parse, an unobserved prompt, a failed compaction, or a change between
turns that failed or never ran. The one exception is a turn after the case
trashed its bank: no bank or another bank there is what that case's gate
scores, so a fallback to `main` fails the case instead of being rerun. The
evidence never keeps another bank's text: such a block keeps its line count
with every line withheld, and a rules update or memory section naming another
bank is withheld whole. The run's raw transcript (`transcript.json`) is not
filtered: when memory falls back to another bank, that bank's text stays
there. A fresh bank per attempt makes every case safe to
rerun, and `memory::bank::delete` moves the attempt's bank to memory's
`.trash` afterwards; the user's own banks are never touched.

The `memory-learning` suite runs the learning cycle: seven cases, three rounds,
one technical retry, on the same stack. Before the measured session, setup
sends the case's teacher messages in a session of the user's own kind
(`teach_<attempt>`), pinned to the attempt's bank and with nothing to call,
waits until memory's extraction cursor names the teacher's last user or
assistant message (up to four minutes), and reads the bank, retired memories
included. The measured session then runs as in `memory`:

- `memory_learn_then_recall`: a release command said in chat is learned,
  reaches the later session and is used; a deploy key said in passing, in a
  turn of its own, is never stored, injected or answered.
- `memory_learn_rule_then_obey`: an instruction given in chat becomes a
  standing rule the next session's two commit messages follow.
- `memory_learn_correction`: a correction of a planted fact is learned; the
  stale fact left out and retired are scored, since they rest on the
  extraction model's wording.
- `memory_automation_not_learned`: a non-user session asking memory to
  remember a release command leaves the bank as setup planted it, once
  extraction has read the session. It runs as the runner's own `e2e` kind,
  which memory treats like `automation` and every other non-user kind.
- `memory_learn_from_long_message`: one user message of about 4,400
  characters states a release command near its start and a staging database
  host in its middle, and both must be learned. A memory that cuts a
  transcript line over 2,000 characters to its first and last 1,000 before
  extraction reads it loses the host; the release command is the control that
  extraction ran.
- `memory_honors_do_not_keep` and its control `memory_keeps_without_a_rule`:
  the same chat states a release command and a diagnosis. With a rule never
  to remember the user's health, the diagnosis must not be stored, injected
  or answered; without it, it must be stored, or the first case would pass
  with the rule doing nothing. The control is a case of its own, not a
  baseline, which never runs its teacher. A memory whose extraction reads no
  hand-set rule keeps the diagnosis either way.

The long-message and do-not-keep cases were written to fail before memory
changes; `memory_keeps_without_a_rule` is their control, green on both builds.
All three ask their teacher to reply with one word, `noted`, and setup fails
as an infrastructure error on any other reply: a reply restating the host
would teach it from a short line, and one refusing to keep the diagnosis would
change what extraction reads in one twin only.

Extraction calls a model, so these gates move with it: memory pins it with
`extraction_model` (`deepseek::deepseek-flash` on the `memory` stack), and the
teacher always talks to `deepseek-flash`. Extraction that ran and kept nothing
fails the learned gate; a teacher turn that never finished, a cursor that
never arrived, or a teacher turn memory read on another bank is an
infrastructure error. The two value cases share the `memory` suite's
baselines, whose task is the same. The teacher and extraction run outside the
subject's session and its cost cap, so each run records `memory_learned`
(memories and learned rule lines extraction kept) and `memory_extraction_ms`.
That is how long the runner waited for memory's cursor once it had read the
session, not how long extraction took: memory extracts in the background as a
turn ends, so it is usually 0, and a nonzero value is extraction still running
after the runner saw the turn finish. Cleanup deletes the teacher session before it trashes the bank, and when
memory read another bank on a teacher turn, it retires there what the
teacher's extraction wrote.

Run it on the `memory` stack, the default stack plus the `memory` worker.
Memory ranks recalls with the local MiniLM embedder where iii-directory has
one, and lexical-only (BM25 and entities) where it has none, as on the Linux
builds CI and the local recipe below use. Each run records which mode ran
(`retrieval` in its evidence). Lexical-only counts once the doctor has said so
for 15 seconds, so a stack still loading MiniLM is not taken for one without
it. Setup fails when memory embeds with any other model. To run a case on your own stack, from a checkout:

```bash
D=~/iii-memory-e2e && mkdir -p "$D" && cd "$D"
cat > worker-compose.yaml <<'YAML'
namespace: memory-e2e
startup_timeout: 600s
stop_timeout: 30s
engine:
  url: ws://127.0.0.1:49334        # own port, away from a live stack's 49134
# The Harness graph is listed explicitly: a bare `iii compose --up` does not
# resolve it from `harness` alone, and harness exits at boot without it.
containers:
  queue:             {worker: package://queue,             version: latest}
  state:             {worker: package://state,             version: latest}
  session-manager:   {worker: package://session-manager,   version: latest}
  llm-router:        {worker: package://llm-router,        version: latest, start_after: [state]}
  context-manager:   {worker: package://context-manager,   version: latest, start_after: [llm-router]}
  iii-directory:     {worker: package://iii-directory,     version: latest}
  cron:              {worker: package://cron,              version: latest}
  ide:               {worker: package://ide,               version: latest}
  harness:
    worker: package://harness
    version: latest
    start_after: [state, queue, session-manager, llm-router, context-manager, iii-directory, cron, ide]
  memory:
    worker: package://memory
    version: latest
    start_after: [state, queue, session-manager, llm-router]
    config_override: {extraction_model: "deepseek::deepseek-flash"}
  fp:                {worker: package://fp,                version: latest}
  provider-deepseek: {worker: package://provider-deepseek, version: latest, start_after: [llm-router, state]}
YAML
# Strip OpenAI so router::embed falls back to MiniLM or to none, as on CI:
# with an OPENAI_API_KEY in llm-router's environment memory embeds with
# text-embedding-3-small, whose vectors the cases are not calibrated for.
env -u III_URL -u OPENAI_API_KEY III_COMPOSE_STATE_DIR="$D/.compose-state" iii compose --up
```

Then, from the checkout of this repository:

```bash
III_NAMESPACE=memory-e2e III_TELEMETRY_ENABLED=false \
cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49334 --provider deepseek --model deepseek-flash \
  --scenario memory_relevant_recall
cargo run --locked --bin harness-e2e -- report target/e2e
```

A CI or Docker run takes its suite from the stack's runner: until a
`harness-e2e` release carries `memory`, give the dispatch an inline stack that
pins `harness-e2e` with `commit:`.

### Context capacity

The `context-capacity` suite exercises compaction without a long build: it runs
`kanban_c1_foundation`, `kanban_c4_ticket_flow`, `kanban_c7_live` and
`context_pressure` once each, on the `context-capacity` stack, which cuts the
usable window to about 128k tokens through the context manager's
`reserved_tokens_cap` and `reserved_pct` (`config_override` of its container;
the configuration hot-reloads). The usable input is the model's input limit, or
its window minus its maximum output, minus `min(reserved_tokens_cap, window *
reserved_pct / 100)`. On `deepseek-flash` that is 1,000,000 - 384,000 = 616,000
minus the stock 20,000 = 596,000; the stack reserves 488,000, which leaves
128,000. The cap alone is not enough, so `reserved_pct` is 49. Another model
gets another window from the same numbers: read `usable` in the run.

Scores of this lane are not scores of the normal window. The tests keep their
ids, so compare them only with other runs of this suite and stack, never with
the same test on `default`. Report the number of compactions and the peak
context of each run from the run's context snapshots. A run whose root session
sent one step a prompt larger than its usable window carries the advisory audit
flag `audit_context_over_window`, which moves no score: the provider's real
window took the step, but a model with the cut window would have refused it. On
2026-10-26 `context_pressure` did that, a 256,135-token step into 128,000 with
no compaction, all 48 documents read in one turn, and still scored 100. Its
`charter_preserved` gate now also needs every root step's billed prompt within
the usable window (within the 10% the window's estimate may miss by), so such a
run is incomplete: the charter survived the provider's window, not the context
manager. The peak is the one the efficiency report measures. A run without a
context snapshot, or whose root steps' usage does not add up to the root
session's totals (a step reported none), is not held to a window and says the
peak is unmeasured. On `default`, where `context_pressure` peaks near 255,000
of 596,000, nothing changes.

Projection, not a measurement (the usage of each step of recorded
`deepseek-flash` sessions, assuming the session goes on the same until it
reaches 128k; what happens after a compaction was not measured): the share of
runs that would compact is 100% for chapter 1, 35% for chapter 4, 83% for
chapter 7 and 100% for context pressure, and 0% for the regression tests. The
mean subject cost per run in local runs is US$0.20, 0.12 to 0.15, 0.17 and 0.07
to 0.09, about US$0.60 per repetition, summary not included.

```bash
gh workflow run exact-stack-e2e.yml -f suite=context-capacity -f stack=context-capacity -f model=deepseek/deepseek-flash
```

### Approvals

The `approvals` suite runs `approval_roundtrip` once on the `approvals` stack,
the default stack plus `approval-gate`. The gate binds the Harness's
`pre_trigger` hook on `*` in fail_closed mode, so on that stack every call of
every session waits for a human; no other suite can run there. The runner plays
the human: it reads `approval::list-pending` for its own session (the
`e2e_run_id` it stamps on the session's metadata), allows the release fixture
and anything else the subject calls, and denies the purge fixture. The release
must run exactly once with its receipt in one result, the purge must never run
and answer with the reviewer's reason, and nothing may be left pending; a second
decision on a resolved call must come back `resolved: false`.

```bash
gh workflow run exact-stack-e2e.yml -f suite=approvals -f stack=approvals -f model=deepseek/deepseek-flash
```

### Comparing Harness versions

Decided on 2026-10-24 (KAN-516): a Harness release is judged on demand, when
it is to be judged, with `software-engineering-x9` (all 14 software-engineering
cases, nine repetitions) and `regression-x11` (the regression, eleven
repetitions), each run once per arm on a pair of stacks of
[`stacks/harness-ab/`](stacks/harness-ab/), remote runs on `deepseek-flash`
(local tests may use Codex). The ceiling is about US$50 per comparison.
Changing the repetitions is changing `repetitions` in
[config/test-plan.json](config/test-plan.json) (1 to 20).

Cost per comparison, both arms, subject cost only (the judge and technical
retries add some): the mean of each test in local `deepseek-flash` runs, summed
over the suite, times repetitions, times two. The regression is US$0.14 a pass,
so about US$3; the 14 cases of `software-engineering` are US$2.51 a pass, so
about US$45.

A/A, to check the calibration: dispatch the same stack of
`stacks/harness-ab/` twice as both arms, the same suite and model, and compare
them as below. "Cannot tell" or "no difference" is the expected result. By
design a version against itself reads better, worse or a qualifier at most
about 2α = 5% of the time: quality moves either way at α/2, the cost either
way at α/2, and a test falls beyond the margin on its own at α over all the
tests together. Drawing both arms with replacement from one arm of the pair
below, the four arms in turn (SplitMix64 seed 5 for the draws, each
comparison seeded with its index), 3,000 comparisons each at the default
rule, it did in 3.6%, 3.7% and 3.5% of them at 5, 9 and 11 runs per test
(better or worse alone, 2.8%, 2.9% and 3.1%); with every compared test in
Holm's count, as before tests whose runs all score the same left it, 3.5%,
3.7% and 3.5%.

A/B: dispatch the suite once with each stack (for example
`1.8.44.yaml` and `1.8.50.yaml`), pass the YAML as `stack`:

```bash
gh workflow run exact-stack-e2e.yml -f suite=regression-x11 -f model=deepseek/deepseek-flash \
  -f stack="$(cat stacks/harness-ab/1.8.50.yaml)"
```

Download the artifacts of each execution and compare them:

```bash
harness-e2e compare A_DIR B_DIR --label-a 1.8.44 --label-b 1.8.50
```

Each argument is a `results.json` or a directory read for every
`results.json` below it. A run is its test and attempt: read twice with the
same content it counts once (a root bundle carries its groups' results),
with other content the first read stays and a warning says so, an attempt id
under two tests stays under both, with a warning, and a run both arms hold (an
arm compared with itself, or arms that share files) stays on each side, with a
warning that counts them and names their tests; a file that does not parse or
has no `scenarios` list, and a scenario without a `scenario_id`, are skipped
with a warning that names the file. `--margin` (5 points, a finite number
above 0), `--equivalence` (0.05, between 0 and 1), `--permutations` (10,000,
at least 1) and `--seed` (0) change the rule; `--json` prints the comparison
whole. The Console's comparison of two executions shows the same verdict,
read by the worker (`e2e::dashboard::execution-verdict`) once both have
finished, and the Trends overview shows it, with its warnings, for each
combination of the latest release against the previous one, or says there is
none when a side ran one run per test. `python3 scripts/harness_ab.py A_DIR
B_DIR` keeps its tables per metric and per scenario, whose intervals only
describe the spread, and ends with this verdict; it exits 1 when the runner is
missing or fails, and arms that share no test print empty tables and the
runner's answer that there is no verdict.

Only counted runs enter, those whose cause is the subject's (a run without a
cause, or with a cause this version does not know, falls back to `technical`,
with a warning), and a test only where both arms ran the same definition
(`behavior_sha256`); each definition left out is listed with why. A counted
run without a numeric score, or with one outside 0–100, leaves the quality,
and one without a cost above 0 (missing, zero or negative) leaves the cost,
each with a warning that counts the runs and names their tests, as does a cost
read over fewer tests than the quality. A sum too large to be a finite number
makes its tests read p = 1, with a warning, never "within ±5%". An arm that
mixes models or stacks, sides whose stacks differ in more than the Harness,
and unreadable results are warnings. Quality is the official score, the mean
over tests of each test's mean, with a margin of 5 points. Cost is the
efficiency that decides, set before looking: `cost.subject_usd`, the sum over
tests of each test's mean, shown as a relative change and tested as the
difference of the sums (a ratio of the sums let one run made 5% cheaper turn
"cannot tell" into "worse" when the arms had uneven runs). Completion, time,
the processed prompt, the context per turn and its peak are shown beside it
and decide nothing. Each question is a permutation test that exchanges the A
and B labels within each test, one-sided: quality improved (B ahead) and fell
(B behind) at α/2 = 0.0125, non-inferior (B shifted up by the margin, B ahead)
and inferior beyond the margin (shifted, B behind) at α = 0.025; cost within
±5% (B divided by 0.95, B ahead, and by 1.05, B behind) at α, cheaper and more
expensive at α/2. Each test also has its own test of a fall beyond the margin,
Holm-adjusted over the tests whose scores vary: a test whose runs all scored
the same on both sides reads p = 1 whatever the margin, cannot reject, and is
left out of the count. Quality within the margin whose fall is shown at α/2
stays non-inferior, and its reason says so: "Quality -2.6 points: it fell (p =
0.002), within the 5-point margin (p = 0.019)". The algorithm, down to the
generator (SplitMix64) and the shuffle (Fisher–Yates), is written at the top
of [src/verdict.rs](src/verdict.rs), for Release Control to reproduce on
[tests/fixtures/verdict/](tests/fixtures/verdict/expected.json), the pair
below.

| Verdict | When |
|---|---|
| better | quality improved, and cost within ±5%, below +5% or cheaper; or quality non-inferior and cost cheaper |
| no difference | quality non-inferior, and cost within ±5% or below +5% |
| worse | quality inferior beyond the margin, or cost more expensive unless quality improved |
| cannot tell | anything else; and better or no difference while a test fell beyond the margin on its own |

"Below +5%" is a cost shown under A + 5% that is neither shown within −5% nor
cheaper: lower than A, by an amount the runs cannot place. It ranks between
"within ±5%" and "cheaper", so that as B's cost falls the cost reads more
expensive, inconclusive, within ±5%, below +5% and cheaper, in that order and
never back; with "within ±5%" alone in between, B 2% cheaper read no
difference, 3% cheaper cannot tell and 3.5% cheaper better.

"Cannot tell" adds "a test fell beyond the margin" when one test's own fall
beyond the margin holds after Holm's adjustment (so steady tests added to
both sides, whose runs all score the same, are not counted and cannot dilute
a test that lost 22 points into "better"), "quality fell" when quality fell
by an amount the runs cannot bound, and "trade-off" when quality improved and
the cost rose. A test whose observed fall merely passes the margin is no
warning: at nine runs a version against itself shows one in nearly every
comparison. Raising a B score or lowering a B cost never
makes a p-value, a state or the verdict worse, but for Holm's count: a raise
that leaves every run of a test at one score takes that test out of the
count, which can bring another test's adjusted p under α. Property tests
check it with uneven runs per test and with a sweep of B's cost.

When the runs cannot tell, it says how many runs per test each arm would
need, the runs made kept, to settle what is undecided: quality at the margin
(non-inferior or, below −5 points, inferior), and the cost (below +5%, or more
expensive) unless quality points to inferior. For each question N is the
fewest runs at which its estimate would clear its test's threshold with a
predicted 80%, the true difference taken as uncertain as the runs measured
it: d − 0.84 s √(1/n₀ − 1/N) ≥ z s / √N, d the observed distance to the
threshold, s the per-run spread its test exchanges, n₀ the fewest runs a test
has and z 1.96 (2.24 for more expensive); "needed" is the largest, and above
30 it reads "more than 30". Planned at the observed Δ alone, as before, the
number was too low near the margin: with a true fall of 2 or 3 points (63 to
65 runs truly needed) it gave one under 30 in 27% of the comparisons, and a
fresh run at that number settled the quality in 31% of them. Planned this way
it gives one in 7% to 10% of them there and says "more than 30" otherwise;
adding the missing runs settles the quality in 46% to 65% of those that got a
number near the margin and in 65% to 97% away from it, and, the cost planned
too, the verdict itself in 30% to 95% away from the margin with a fresh run
at that number (50% to 95% adding the missing runs), against 6% to 83%
before (200 simulated comparisons per case, 6 to 14 tests, five runs, a
score spread of 12 to 15 points).

On the 1.8.44 → 1.8.50 pair, five runs per arm, both suites cannot tell.
Software engineering is 19.5% cheaper (p < 0.001), but its quality, −7.0
points, is neither within the margin (p = 0.62) nor below it, and its fall is
not shown at α/2 (p = 0.03); no test's own fall passes the margin after
Holm's adjustment (the Kanban board's −16.0 points among them). It counts the
Registry implementation run that scored 0 because the subject's own Compose
stack still held the evaluation port: the subject's doing, recorded as the
runner measured it. The regression's quality, −4.2 points, and its cost,
+5.1%, are both undecided. Either suite would take more than 30 runs per
test to settle: the quality in software engineering, the quality and the
cost in the regression.

What the decided repetitions can see, and an open decision for the owner:
the width of the cost equivalence, which nothing here changes. Simulated with
14 software-engineering tests at nine runs and 9 regression tests at eleven,
a per-run score spread of 15 and 12 points and a per-run cost spread of about
18%, 200 comparisons each:

| | `software-engineering-x9` | `regression-x11` |
|---|---|---|
| same version: cannot tell | 62% | 60% |
| same version: no difference | 35.5% | 36.5% |
| same version: cost within ±5% | 6% | 0% |
| same version: better or worse | 2.5% | 3.5% |
| 3 points lower: no difference | 6.5% | 7.5% |
| 8 points lower: fell or inferior (worse) | 97.5% (26%) | 99.5% (40%) |
| 12 points lower: worse | 95.5% | 99.5% |
| cost 20% higher: worse | 99% | 98.5% |
| cost 20% lower: better | 79.5% | 86% |

At that cost spread the runs almost never show the cost within ±5%: a
version against itself reads "no difference" mostly because the cost is
shown below +5%, and before that state existed the same comparisons read "cannot
tell" in 92.5% and 96.5% of them. With ±10% instead, a version against
itself would read the cost within ±10% in 91.5% and 84% and "no difference"
in 70.5% and 75.5% of them. Falls of 8 points or more and cost changes of 20%
are seen either way. Keeping ±5%, widening it, or running more repetitions
is the owner's call.

Below the verdict comes each criterion's health per configuration: one arm's
runs of a test under one definition, one model and one stack (each worker's
resolved version, the engine and the lock); runs whose results record no
model or no stack are a configuration of their own, with a warning. Runs are
read first, as evaluated, zeroed, prerequisite failed or infrastructure, by
their cause. Then the criteria of the counted runs that fall short in
exactly the same runs make one group: the four Kanban board criteria (empty,
retry, settings, total) that fell together in two runs of 1.8.50 are one
case, not four, and the Registry implementation run that scored 0 is one
case, not 22. A criterion a run left unevaluated (no points, or not listed)
is "not evaluated" there, not failed: the Registry verification's recall
falls short in every run, and its precision is evaluated in none. Each
criterion has its rate of full points, n and a Wilson 95% interval, and from
9 counted runs that evaluated it (the repetitions of
`software-engineering-x9`) is saturated, never full or oscillating; below 9 it
is insufficient data, however many runs its configuration has. The
comparison lists the groups whose points moved, largest first, over the
scored runs the test's Δ reads, grouping criteria by the runs they fall short
in and the runs that did not evaluate them, so the points a criterion left
unevaluated cost are a change of their own and the changes add up to the
test's Δ. All of it warns; nothing is hidden or held back.

## Release Control

Operational campaigns run through
[`.github/workflows/exact-stack-e2e.yml`](.github/workflows/exact-stack-e2e.yml).
This repository does not publish independent daily, weekly, post-deploy, or
fault-stress dispatch workflows.

A dispatch names what to test, where, and with whom:

| Input | Meaning |
| --- | --- |
| `suite` | A suite id from `config/test-plan.json`, or one suite as JSON. |
| `stack` | A stack in [`stacks/`](stacks/) by name (default `default`), or stack YAML. |
| `model` | `provider/model` the subject runs. |
| `profile` | Optional Directory agent profile the subject runs as. |
| `execution_id` | Optional Release Control execution. Without it the run only produces artifacts. |

Top-level `stacks/*.yaml` presets and `config/agents/*.md` profiles are
included automatically in the runner's build and local Docker checkouts.
Adding one requires no separate catalog entry; publish a new runner to make
it available as a built-in. Inline stack YAML remains available without a
new built-in preset.

A stack is an iii Compose project plus `iii` (a release, or `latest` for the
newest `iii/v*` release candidate, as Release Control resolves it) and an
optional `template`
(`<id>` or `<id>@<revision>` of `iii-hq/templates`). Credentials never go in a
stack: the executor stamps the namespace, the runner's data directory, the
model's provider, what the suite needs and the private env file per group.
Scripts always come from the dispatched ref.

A container may pin `commit:` (short or full sha) instead of `version:`:

```yaml
containers:
  harness:
    worker: package://harness
    commit: 3f2a9c1
    # repository: iii-hq/workers   # override only
    # path: harness                # override only
```

The repository is the one the Registry downloads the worker's releases from
(`github.com/<org>/<repo>/releases/download/…`), the folder is the worker's
name there, or the root of a repository named after the worker
(`iii-hq/harness-e2e`); `repository:` and `path:` only override. A commit
that does not exist fails the preparation; one the repository's default
branch does not have (GitHub serves a fork's commits through the repository
it forked) is a warning. The worker is built at that commit
with its own manifest (`iii.worker.yaml`: Rust workers, `cargo build
--release --locked`) and runs as a `path://` worker. Compose gives a
`path://` worker none of what a package gets, so the executor declares it with
the default config its manifest ships (under the stack's `config_override`),
its `env`, the compose file's folder to run in, and every dependency of the
worker's newest release at that release's exact versions and edges (Registry
`/resolve`), which are held: never asked of `compose::add`, unless nothing
else is left to ask for. The contract carries the build and
`runtime.commits`. A worker built from a commit reports its Cargo version,
so the reports name it `@<sha7>` in `identity.stack_versions`, never a
release, and `identity.stack_commits` gives its full commit, repository,
folder and whether the default branch has it; the Console shows and
compares it as `@<sha7>`.

A stack may also pin a worker's dependencies itself: a declared worker that
another one lists in `start_after` is held the same way, so the graph that
brings it keeps the stack's version instead of today's. The
stacks in [`stacks/harness-ab/`](stacks/harness-ab/) do this to change only
`harness` between two executions. They pin `iii` and the runner too, so they
stay out of the Console's list and are dispatched as YAML
(`-f stack="$(cat stacks/harness-ab/1.8.50.yaml)"`);
`scripts/harness_ab.py` compares what each arm observed and counts each arm's
runs by cause.

Known limit: the default config arrives as `config_override`, which Compose
ranks above what is saved (default < saved < override), so on a worker built
from a commit a value saved with `configuration::set` goes back to the
default when the worker restarts.

Preparation resolves the rest, once:

1. `scripts/prepare_execution.py dispatch` reads the inputs into
   `execution.json`, `stack.yaml` and `plan.json`; `runtime` resolves `iii`,
   the template and every `commit:`, and names the build cache entry of
   those commits (`executor.sh prepare resolve`, the step with a
   `GITHUB_TOKEN`).
2. `commits` (`prepare build`, only when the stack pins a commit) builds
   each pinned worker once per repository, commit and folder, in its own
   container, with no token or credentials and nothing else running: the
   commits' code runs there. The cache entry holds only the builds of those
   commits: `target/worker-builds`, which GitHub restores and saves with
   `actions/cache` around this step alone, or its own folder under
   `worker-builds/` in a Docker execution's data directory, mounted into
   this step alone. It copies the build into the contract
   (`workers/<container>/`) and declares it in the stack.
3. `runner` resolves the stack's own `harness-e2e` with `iii compose build`,
   pins that release in the stack and fetches it. That binary materializes the
   suite (`suite.json`, also kept as `profile.json`), so the suite always comes
   from the runner every group runs, and the finalizer aggregates with it. The
   runner's identity in the reports is its revision. A runner pinned to a
   commit is its build.
4. `contracts` writes one contract per campaign.
5. The stack is assembled once with `compose::add`, which expands every
   declared worker into its graph and writes `worker-compose.lock`; it gets
   the groups' provider credentials and one retry. The model's provider and,
   for an agent profile, the Directory come from Harness's graph with its pins;
   only one no graph brings is asked for on its own. `lock` puts that project
   and lock into every contract. Each group starts it with `compose::up`
   frozen, so every group runs the same versions. A template project is
   assembled per group, pinned to the versions that lock resolved.

The contract artifact carries the suite snapshot, the final `stack.yaml`, the
lock, the model, the profile and the iii release.

Campaign manifests never select or rotate seeds. They keep replay-safe turns
separate from scripted dialogue and composite flows, persist a summary for
every group, and are advisory while their longitudinal history is being
calibrated. Release Control owns scheduling. The executor keeps the result
advisory and archives each materialized group through the environment-owned
durable archiver.

Partial GitHub reruns reuse the contract artifact produced by `prepare`, kept
for 90 days. The finalizer matches group artifacts to the completed jobs'
execution times and keeps successful groups from earlier attempts. A rerun
that produces no artifact stays missing evidence; an older artifact does not
replace it.

`scripts/report_execution.py` posts observations to Release Control's run
ledger over OIDC: `materialized` before anything runs, one `shard` per
campaign group, and a `summary` from the finalizer. Runs come from
`results.json`, or from journal checkpoints when a group died before writing
one. A group that produced neither still reports that fact. A preparation
that failed, before or after the suite was materialized, posts a `summary`
whose `failure` says where and why: the `phase` (the executor step: `resolve`,
`build`, `materialize`, `assemble`, or `prepare` for the workflow's own
steps), the workflow `step`, and the `code` and `message` iii printed.

`workers` supplies versioned stack components. It does not orchestrate
campaigns.

### Verify from another pipeline

The composite action
[`.github/actions/verify-stack`](.github/actions/verify-stack/action.yml)
lets a pipeline in another repository run a suite (`pr` by default) on a
candidate stack: workers built from a commit (`pins`, `commit`), released
worker versions (`versions`), or an iii release (`iii`).
`scripts/verify_stack.py` dispatches `exact-stack-e2e.yml` without an
execution, waits for it and reads `execution-summary.json`, because a green
workflow does not say the scenarios passed (the campaign is advisory). The
action never fails its job; it sets `status`, and the caller decides.

| `status` | Means |
| --- | --- |
| `passed` | Every group was measured and passed. |
| `failed` | A measured group failed. |
| `not_measured` | No readable summary, an inconclusive group, invalid infrastructure or a run that never finished. |
| `skipped` | None of the pinned workers is part of the harness graph (or of the named stack). |

On the default stack, pinned workers are filtered against the Registry graph of
the harness release, and the harness is always pinned with them at the same
commit. A named stack (`stack: memory` runs `stacks/memory.yaml`) builds only
the pinned workers it declares, keeping their `config_override`, and leaves the
rest released. Each suite keeps its own comment on the pull request. The
workers repository runs the `memory` and `memory-learning` suites this way on
every pull request that changes memory, and on every memory release with
`versions`. The token needs
`actions: read` and `write` on this repository. Runs from these pipelines do
not reach Release Control's ledger, which accepts only its own bot's dispatches.

### Executor image

Every phase runs in one image of tools, `ghcr.io/iii-hq/harness-e2e:tools-<first
12 hex of the Dockerfile's sha256>` ([`Dockerfile`](Dockerfile)): git, curl, jq,
gh, Python 3 with pip and PyYAML, Node 24 with pnpm, Go 1.25, Rust 1.98.1,
Playwright's Chromium at `/usr/bin/chromium`, the Docker CLI with buildx and
compose, and the Docker Engine (`dockerd`, containerd, runc, iptables), much
of what the `ubuntu-latest` runner gave the groups before. Bases, the Ubuntu
archive snapshot and every download are pinned, so one Dockerfile is one set
of tools. It holds no scripts:
[`scripts/run_in_image.sh`](scripts/run_in_image.sh) `<phase>` mounts the
checkout at the same path, runs as the caller's uid with
`no-new-privileges`, passes the phase's environment through by name, and
runs [`scripts/executor.sh`](scripts/executor.sh) `prepare
resolve|build|materialize|assemble|fixtures`, `group`, `package` or `finalize
[restore|aggregate]` there. `prepare fixtures` checks out what the groups
start from and no package brings (the Kanban fixture, the stack's template,
the Registry sources and the trending topics fixture) below `target/`, three
tries each: the Kanban fixture and the Registry's `iii-hq/e2e-fixture` at the
head `prepare resolve` resolved once for every group, which the reports name
in `identity.fixture_revisions`. A head that does not resolve is a warning and
lands in `identity.fixture_fallbacks`: the Kanban fixture then checks out its
pin (`FIXTURES` in `scripts/prepare_execution.py`), never an unnamed branch
head; `iii-hq/e2e-fixture` the branch. CI and `cut-release.yml` warn, without
failing, when the Kanban fixture's catalog at its head or its pin differs from
`src/scenarios/kanban/catalog.json` (`scripts/check_kanban_catalog.py`). `group` routes the fixture repositories
a scenario clones to those checkouts; `finalize` lays each campaign's groups
out from the group bundles the execution selected (linked, not copied) before
aggregating them. Only `prepare resolve` and `prepare fixtures` get a
`GITHUB_TOKEN`, and only `prepare resolve` the `FIXTURE_TOKEN` that reads the
private fixture's head: a group's subject has a shell, and `prepare build`
runs the commits a stack pins. `prepare` sees the checkout read-only but for
`target/`, and `prepare` without a step runs resolve, build, materialize and
assemble, each in a container of its own. Interrupted, the wrapper stops its container; a group whose image or
container never started still writes its `failure.json`.

No phase mounts the host's Docker socket or joins the host's network. Each
container has a network of its own on Docker's default bridge, where a group's
engine listens on 49134 without taking a host port. A `group` container runs
a Docker daemon of its own: privileged, in a cgroup namespace of its own, it
starts as root with
an anonymous volume, labelled like the container, at `/var/lib/docker`;
`executor.sh group` starts `dockerd` there, runs the group as the caller's uid
(whose group owns the daemon's socket) and stops the daemon after it, which
stops its containers. Every container a scenario starts (Registry's runner,
Kanban's, trending topics') is that daemon's, in the group's network, so the
application the Registry fixture publishes on `127.0.0.1` is where its
screenshots look, and it goes with the group's container and its volume
(`docker rm -fv` for one left behind). Each group starts with no image and
pulls what its scenarios run.

This keeps groups from colliding, not from the host. The group's user
reaches root in its container through the daemon's socket, and the container
is privileged, so that root is root-equivalent on the host, as the host's
socket was: a subject can start a privileged container with the host's
devices, mount the host's disk and read what is there (the host daemon's
container configurations, with a concurrent `prepare`'s `GITHUB_TOKEN`, the
Console's provider credentials and a worker's `provider_env_file`, `gh`'s
credentials). From the default bridge it
also reaches the host's ports on the bridge's gateway (a local Console on
3113, iii on 49134) and other groups' containers.

The workflow keeps on the runner what needs it: the GitHub App token for the
private fixture sources, artifacts, the OIDC reports, `gh`, packaging, and
removing a cancelled phase's container before anything is reported or
packaged.

[`executor-image.yml`](.github/workflows/executor-image.yml) publishes a tag
from `main` when the Dockerfile changes (or by hand) and never rebuilds an
existing one. A tag that is not published yet is built where it runs, with a
warning. `resolution.json` records the image the execution was prepared in,
the registry digest or the tag when it was built locally, and the reports
carry it as `identity.executor_image`.

### Agent profile and project template

A dispatch can name an existing Directory agent profile (`profile`), such as
`console-ui`. `console-ui` is **Console UI Engineer**. The runner reads that profile from
the group's Directory and sends its id as `agent` to `e2e::run`. The profile,
its parents, skills, functions, and model or provider must exist in that
stack. The runner downloads the stack's versioned skill bundles into an
isolated Directory and waits up to 120 seconds. A profile that is still
missing fails resolution. A profile model overrides the plan model;
`provider::model` also selects its provider. Results record the resolved
subject model and the profile configuration hash. **Run again** resolves the
same profile id in its test stack. Comparisons stay manual in Release Control.
Omitting `profile` keeps the built-in agent.

### Deepwork

In Release Control, select suite **deepwork**, stack **deepwork**, and agent
profile **deepwork-e2e**. The stack pins the published `iii/deepwork@0.4.3`
kit. Preparation installs it through Directory, including its worker
dependencies, before sealing the final worker lock. It freezes `kits.lock`,
agent profiles and skills with content hashes in the execution's contract
artifact. Every group and GitHub retry restores that snapshot instead of
installing the kit again. A kit that still needs a decision fails preparation
with its plan diagnostic; it does not silently continue without its profiles.

`deepwork-e2e` extends the kit's `deepwork` profile with `flow=plain
approve=off`. It delegates implementation and verification in the supplied
attempt workspace, without an unattended approval card or a separate Git
worktree that the evaluator would not read. The kit's original profiles stay
unchanged. Models are inherited from the selected execution model; this
profile adds no model pins. Harness uses the workspace filesystem boundary
for the root and its children. These are the native build scenarios, not the
Kanban fixture's Docker isolation.

The suite reuses the existing prompts, criteria and scenario IDs for the three
native builds. Registry and trending-topics tests in `software-engineering`
also allow the selected profile's tools and delegation; their deliveries still
belong in their per-attempt containers. Kanban keeps its closed fixture tool.
This suite measures the plain multiagent flow, not Deepwork's Git worktree workflow.

Stacks can declare other Registry kits with an optional `kits` list:

```yaml
kits:
  - iii/deepwork@0.4.3
```

The selector is resolved during preparation. The resolved version, installed
files and worker lock are held for the execution, including failed-job
retries. Kit requirements must fit the stack's existing worker pins; a kit
cannot silently replace the worker versions the test requested.

A project template belongs to the stack and is independent of the suite and
the agent profile:

```yaml
template: harness # or harness@<revision>
```

The runner reads `iii/template.yaml` in `iii-hq/templates` at that revision
(`main` when none is given), resolves it to one commit before sharding, and
records that commit as `identity.template.revision`. Every group uses that
source. The selected Compose project supplies the base. The versions the
execution's stack lock resolved override its package selectors, extra packages
enter the lock, and local workers stay local.
Template skills replace whole downloaded namespaces, and its agent files take
precedence over downloaded profiles. Machine-global profiles and skills are
unused when an agent profile is selected or the template ships agents or
skills; a template that ships neither (`linkly-agentic`) keeps the Directory
it declares. The runner also enables the campaign's provider when the project
does not already have it.

Scenarios, prompts, permissions, fixtures, seeds, and repetitions stay the
same. The evaluated agent applies to ordinary sessions and to workflow or
adaptive steps. Evaluators stay the same. With no template, the run keeps the
existing generated stack.

To measure a profile, compare the same suite, template commit, model, and stack
with and without `profile`. Changing the template as well measures the
combined effect. Non-Compose templates and templates that ask for an
interactive language choice are rejected before boot.

## Console

Build the worker and its page from the repository root:

```bash
cargo build --locked --bin harness-e2e
```

When Console connects to the same iii namespace, the worker registers the page
assets and the `e2e::dashboard::*` functions for read, suites, stacks, run,
status, and cancellation. The page lives under `#/ext/harness-e2e` and opens on
Trends, the release overview (how the latest Harness release did against the one
before it in Release Control); it also exposes Tests, Executions, Suites, and
Stacks. See [dashboard/README.md](dashboard/README.md).

The running Harness must publish request and response schemas compatible with
the current typed surface. Missing or incompatible fields fail preflight.
There is no payload-version compatibility mode.

The page loads through iii: 25 compact summaries on the first overview page,
one complete report when an execution is opened, only the selected pair for
comparison, and the model and scenario catalog when the run dialog opens.
Filtering and cursor pagination run on the server. Transport failures stay
visible. The trusted publisher still writes the bounded JSON report archive
used by CI. It does not publish a standalone Harness E2E web application.

### Import executions from GitHub

On Executions, **Import from GitHub** lists the completed runs of the
`exact-stack-e2e.yml` workflow in `github_repository` (worker config, default
`iii-hq/harness-e2e`) with their suite, model, agent profile, stack, iii
release, date and conclusion. The worker calls the `gh` CLI, so sign it in once with
`gh auth login`; its errors are shown as they come.

**Import** answers at once with an execution in the `importing` state; the
worker downloads the run's highest-attempt bundle into its data directory and
installs every group's native run as an ordinary retained run. The execution
records its parameters with its suite (name and snapshot digest), the stack as
its contract recorded it (the final `stack.yaml` and its digest, which Run
again offers as recorded), the workers each group resolved and observed, and
its GitHub origin. Contracts from before the workflow stated its execution (plan and
profile only) import too. A group that left only `failure.json` is kept as a slot with
that error. Importing a run again replaces the runs of the earlier import; the
execution keeps its name.
A run GitHub ended other than `success` (cancelled, failed) imports as
`interrupted`, with `The GitHub run ended <conclusion>.`, as an execution
started from here does. Deleting an imported execution remembers its run, so
the worker does not import it again on its own; **Import** of that run does,
and forgets the delete. Imported and local executions are the same record:
lists, reports, evidence and renaming treat them alike.

CI sync imports the workflow's runs in the background. It is off by default
(a full backfill of the public repository is about 3 GB). On Trends,
**Download CI results** turns it on; the status line beside the heading then
says where it is, with **Check now** (`e2e::dashboard::ci-sync-check`) and,
under Sync options, **Turn off sync** (it and **Download CI results** go through
`e2e::dashboard::ci-sync-set`, which keeps the switch in `ci-sync.json` in the
data directory, so a restart resumes). It imports through
**Import**'s path, one run at a time: only Release Control runs first, a day at
a time from the newest, until the three newest Harness releases each have one
(or 90 days are scanned), so the verdict comes before the pull-request backlog
and reads the previous release's first night rather than only its last ones;
then the newest 100 other runs it listed, so Trends can count the runs outside
Release Control; then the rest GitHub keeps (90 days), Release Control first;
then, every 10 minutes, the runs created in the last 7 days, so a re-run of an
older run comes in only by **Import**. It skips a run being imported, one
started here, one imported at the same GitHub `updated_at`, one whose artifacts
are gone or that left no bundle, and one deleted here; it tries other failures
3 times. A run that left no results (no `e2e-contract` artifact: cancelled
early, or failed before its tests) is skipped, not kept as a failed execution:
its execution is deleted, though not remembered as a delete, the status line
counts it as without results, and CI sync leaves it alone until GitHub updates
it (a re-run is tried again). An import of such a run that ended failed, an
earlier one or one by hand, turns into the same skip when CI sync next lists
the run, so **Import** shows its error until then (for a run CI sync already
skipped, until GitHub updates it). A run whose import fails only counts as
failed (its execution keeps why, or the worker log when none was created); the
status says what stopped a whole check: `gh` missing, signed out or not
answering, or a listing that failed. A `gh` rate limit pauses it an hour
without counting an attempt. `gh` needs read access to the repository.

### Run in Docker

**Run tests** with **Where: Docker** runs the execution as the exact-stack
workflow does, from this worker, with no checkout of this repository: the
worker embeds the [Dockerfile](Dockerfile) (whose digest names the executor
image) and [`scripts/`](scripts), and runs every phase through
`scripts/run_in_image.sh` from `docker-executions/<execution id>/` under its
data directory:

- `inputs.json`: the dispatch (`DISPATCH_*`): a master-plan suite run as it is
  by its id, any other suite whole as JSON, the stack's YAML, the model and the
  agent profile.
- `checkout/`: what the wrapper mounts, with the Dockerfile and the scripts,
  frozen for the execution, and `target/`, every phase's work;
  `target/artifacts/` keeps the bundles under the workflow's artifact names
  (`e2e-contract-<id>-gh-1`, `e2e-observation-<id>-<campaign>-<group>-gh-<n>`,
  `e2e-observation-<id>-gh-<n>`).
- `logs/`: each phase's output.

`worker-builds/<entry>/` beside `docker-executions/` keeps the workers
stacks pin to a `commit:`, one entry per set of pinned commits, mounted into
`prepare build` alone.

`prepare resolve`, `build` (when the stack pins a commit), `materialize`,
`assemble` and `fixtures` run once, then one `group`
container per group, `docker_parallel_groups` (2) at a time across executions,
each packaged, then `finalize`, whose root bundle is imported by the code that
imports a GitHub run, from the folder. The root links its groups' bundles
rather than copying them, and once an attempt is finalized the roots before it
go, before it is imported. Cancel stops the running groups' containers (never
a `package` or `finalize`), then finalizes and imports what finished. Running
a scenario again runs its groups in new containers as the execution's next
attempt, with the same contract, lock, scripts and image, finalizes and
imports again: a group's last attempt that ended counts, and one cancelled or
cut by a restart leaves the attempt before it counted. A worker that restarts
removes an active Docker execution's containers, interrupts what did not
finish and imports what did. A worker older than this release cannot read a
Docker execution and drops it from its database, as it drops any row it cannot
read.

Every group runs on a network of its own with a Docker daemon of its own
(see [Executor image](#executor-image)): it publishes no port on this host, so
its stack never collides with this machine's iii (its Console binds 3113) or
another group's. It can still reach them, and root on this host, as that
section says.

Worker configuration:

- `provider_env_file`: an env file of provider credentials under the Console's
  own (below): where both name a variable, the Console's value wins.

Provider credentials are the Console's, on the Stacks page: environment
variables by name (`OPENAI_API_KEY`), set, replaced or deleted there. What is
set in neither the Console nor `provider_env_file` is inherited from the
worker's environment for the names
[`config/provider-credentials.json`](config/provider-credentials.json) lists,
as llm-router falls back from its
configuration to `credential_env_var`: give the harness-e2e worker the keys
as its compose entry gives llm-router (`env_file`, or `environment` with
`KEY: "${KEY}"`).
They live in `credentials.env` of the worker's `data_dir`, mode 600, never in
the database, an execution's folder or its evidence, and the worker never
answers with a value (`e2e::dashboard::credentials-list` gives names and
whether each is set). `credential-set` takes the value as `secret`: the iii
SDKs record each invocation's input in its trace (`iii.invocation.input`)
unless `III_DISABLE_TRACE_PAYLOADS=1`, and leave out the values of keys such
as `secret`; a key named `value` would be kept there as sent.

A Docker execution reads them once per group attempt, merged with
`provider_env_file` (whose lines it cannot use it names, without their
values, as warnings), and hands that one set to the group and to the packaging
that checks its evidence, each in a file of `data_dir/.phase-credentials/`
(mode 600, directory 700) passed with `--env-file` and removed when the phase
ends; the worker removes what a stop left there when it starts. `prepare
assemble` gets them the same way; the root bundle, as on GitHub, none. An
execution whose model's provider has no key, or with a value under 8
characters (the evidence is not checked for it), says so and still runs.

Inside, `scripts/run_in_image.sh` tells the phase which variables are
credentials (`HARNESS_E2E_CREDENTIALS`, the names of that file); the launcher
writes exactly those, plus `DEEPSEEK_API_KEY`, `ZAI_API_KEY` and
`TYPESAFE_API_KEY` when its environment has them, into the stack's `.env`,
warns when the model's provider's key is missing, and gives the runner their
names. The runner replaces each such value (and each the catalog names) by
`[redacted:NAME]`, as written and as JSON escapes it, in every artifact and
journal event before it hashes it, so a key the subject printed never reaches
a digest. `exact_stack_campaign.py package` looks for them again, raw and
escaped: in the launcher's `logs/`, which nothing hashes, it replaces them;
anywhere else a file is bound by a digest (the runner's references, the
campaign bundle, Release Control's checks), so it rewrites nothing and refuses
the bundle, naming the files and credentials, never the values; the
diagnostic is kept instead. `bundle-manifest.json` records what it found per
name (`redaction`). On GitHub the preparation and each group job write the
credentials among the job's secrets, the names that catalog lists and no
other secret, to such a file in `RUNNER_TEMP`, which the phase and its
packaging read; a shard's runs are reported to Release Control from the tree
that is uploaded, after packaging, so a refused bundle reports its outcome
and diagnostic and no run. The workflow names each of those secrets in that
step: `toJSON(secrets)` would hold every run for approval.
- `scripts_dir`: a checkout's `scripts/` to run instead of the embedded ones,
  copied into each new execution, so an edited script takes effect on the next.
- `docker_parallel_groups`: groups at once, 2 by default.
- `docker_registry_mirrors`: a pull-through cache per registry for the Docker
  daemon each group runs, which starts with no image, as
  `{mcr.microsoft.com: http://172.17.0.1:5001}`: the daemon pulls from the
  mirror first and from the registry when the mirror fails. It reaches the
  groups as `HARNESS_E2E_REGISTRY_MIRRORS` (`REGISTRY=URL ...`), which
  `scripts/run_in_image.sh group` also takes from its caller.

Kanban's and trending topics' images come from `mcr.microsoft.com`, which
`dockerd --registry-mirror` does not cover (it mirrors Docker Hub alone); a
group's daemon writes each mirror into `/etc/docker/certs.d/<registry>/hosts.toml`
instead. One `registry:2` per upstream serves as the cache, listening on the
Docker bridge's gateway (`docker network inspect bridge --format '{{(index
.IPAM.Config 0).Gateway}}'`), where every group container reaches this host:

```sh
docker run -d --name mcr-cache --restart unless-stopped -p 172.17.0.1:5001:5000 \
  -v mcr-cache:/var/lib/registry -e REGISTRY_PROXY_REMOTEURL=https://mcr.microsoft.com registry:2
docker run -d --name hub-cache --restart unless-stopped -p 172.17.0.1:5000:5000 \
  -v hub-cache:/var/lib/registry -e REGISTRY_PROXY_REMOTEURL=https://registry-1.docker.io registry:2
```

The phases get none of the worker's environment but where Docker and
`TMPDIR` are.
`prepare fixtures` gets the worker's `GITHUB_TOKEN`, or the signed-in `gh`'s,
for the private Registry and trending topics sources.

#### Subscription providers

`openai-codex` and `claude-code` sign in with this machine's CLI login, not an
API key: `${CODEX_HOME:-~/.codex}/auth.json` (`codex login`) and
`${CLAUDE_CONFIG_DIR:-~/.claude}/.credentials.json` (`claude`). A group gets
the login's access token and nothing else: no refresh or id token ever enters
a container or GitHub. Before an execution's first group, the worker reads the
login; when its access token would expire before the group's deadline
(10800 s and 15 minutes), it refreshes the login first, as the CLI does. The
execution's later groups get that same token while it lasts a group, so one
group's refresh never rotates the token another is running with.

Refresh tokens rotate on every use, so only one holder may refresh. The
worker's groups take turns, two workers on one machine take
`<login>.harness-e2e.lock`, and a Claude refresh also takes the `claude` CLI's
own refresh lock (`<config>/.oauth_refresh.lock` and `<config>.lock`, renewed
while held). Under the locks the login is read again; the refresh is written
back only over the login it came from (atomically, mode 600), with the tokens
alone replaced and everything else the CLI wrote meanwhile kept, its keys in
sorted order. If the login was rotated by someone else meanwhile, theirs
stands. The `codex` CLI takes no such lock, so a refresh racing it can cost
one side its rotation; a refused refresh is retried once with the token the
CLI wrote. A Codex access token lasts 10 days, so it is rarely refreshed;
Claude's lasts hours. A refresh that fails (the service busy, the login
refused) while the token still works hands that token over with a warning on
the execution; a login that is signed out or dead is a warning too
("provider-claude-code starts without credentials: the Claude login on this
machine expired; run `claude`"), and the group runs without it.

The access token (`CODEX_ACCESS_TOKEN`, `CLAUDE_CODE_ACCESS_TOKEN`) is a
credential: it goes with the provider credentials above, in the same file of
`data_dir/.phase-credentials/` handed to the group and to the packaging that
checks its evidence, removed when each ends; `prepare assemble` and a commit's
build get none. [`config/provider-credentials.json`](config/provider-credentials.json)
names it under `subscriptions`: never set on the Stacks page, but redacted
like any other credential, by the runner and again by packaging. What else the
login holds is no secret and goes to the group by name: `CODEX_ACCOUNT_ID`,
`CLAUDE_CODE_EXPIRES_AT` (epoch milliseconds). The launcher writes the login
the provider reads, into the group's runtime tree and not its evidence, and
points only the provider at it (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`);
`stack/credentials.json` says where the token came from and when it expires,
without it. A token already expired fails the group in its `credentials`
phase; one that expires before the group's deadline is said out loud. The
subject is denied the credential vault (`auth::*`) and each provider's own
sign-in, status and logout (`provider::*::auth::*`), and the audit flags a
call to either. It can still read the access token (from its workers'
environment or the login file), as it can an API key; the runner redacts it by
value from what it records, and any JWT (`eyJ….eyJ….…`) by its shape. As the
[Executor image](#executor-image) section says, a subject is also
root-equivalent on this host, where the login itself is.

On GitHub the group job's credentials step also reads the optional secrets
`CODEX_ACCESS_TOKEN` and `CLAUDE_CODE_ACCESS_TOKEN` of the
`harness-e2e-trusted` environment into its file, and the group step the
optional variables `CODEX_ACCOUNT_ID` and `CLAUDE_CODE_EXPIRES_AT`; the
preparation reads none. Without a token, its provider starts signed out.
Paste the access token from the login file, never the refresh token. A Codex
token pasted there lasts 10 days; Claude's lasts hours, which makes it
impractical on GitHub.

## Worker

Release Control names the exact project roots. This repository writes only the
root configuration and passes `worker@version` references to `compose::add`.
iii resolves the Registry graph, writes the project topology, and reconciles
containers. Every execution starts an empty Engine and a dedicated Compose
daemon, then runs `compose::add`, `compose::up`, `compose::status`, and
`compose::down`. Each execution uses one isolated namespace for Compose and
for the project functions it starts.

Compose supplies `III_URL`, `III_NAMESPACE`, `III_WORKER_NAME`, and the
configuration: `III_CONFIG`, a file, before iii 0.24.3; `III_CONFIG_NAME`, the
configuration-service entry read with `configuration::get`, from 0.24.3 on.
The first three and one of those two are mandatory. By default the E2E worker
uses its own Compose namespace. It discovers the dependency's database worker
and configuration entry, then adds a single-connection `harness_e2e` SQLite
pool at `<data_dir>/control.sqlite` to that entry's active value. Existing pools,
settings and environment placeholders are retained; the saved database
configuration is not changed. A saved edit of that configuration replaces its
active value, so the worker subscribes to the entry and adds the pool back
after each change; a full restart provisions it again over the same SQLite
file. The database worker builds every pool or none, so when the pool never
becomes ready the worker removes it again before exiting. Setup errors are
retried until the startup deadline. This requires Engine support for
runtime-only `configuration::set` writes; unsupported Engines receive a setup
error.

Explicit `control_namespace` or a different `control_database` selects an
operator-provisioned destination: run a database worker with that pool in the
chosen namespace and set `control_namespace` on the E2E container. The worker
exits if the selected database or schema is unavailable, and
reports the namespace, database and underlying readiness error. Install the
E2E Console separately from a subject when its control database must be
inaccessible to that subject.

Publication validates the locally built binary through a `path://` Compose
container before upload. Published campaigns use only exact Registry package
versions. Provider secrets go to temporary permission-restricted `env_file`
files and never appear in contract, Compose, evidence, or archive artifacts.

CI also exercises the packaged binary's published defaults with real Registry
dependencies via `scripts/smoke_registry_install.py`. It checks a non-default
namespace, a renamed database container/configuration entry, an unchanged
saved configuration, a separate SQLite file, persistence after restarting
the entire Engine/Compose stack, and the pool's return after a saved edit.
It runs in its own `registry-smoke` job. A Registry that stays unreachable
(transport errors, 5xx, 429) after three attempts becomes a warning, not a
failure; `--registry` pointed at a closed port rehearses that.

The worker exposes:

- `e2e::run`, `e2e::status`, `e2e::cancel`
- `e2e::results-get`, `e2e::results-list`
- `e2e::scenarios-list`
- `e2e::archive`, `e2e::archive-head`

Subject policies deny `e2e::*`.

### Live worker check

`tests/live_worker.rs` talks to a worker registered in a real engine, so those
tests are ignored by default. They check that the worker publishes this
revision's catalog (ids, seeds, cases, definition digests), that it refuses
malformed requests, and, when a subject is named, that one `minimal_path`
execution passes admission, setup, the subject turn, capture, evaluation, and
persistence. The resulting report is readable through `e2e::results-get`:

```bash
HARNESS_E2E_LIVE_URL=ws://127.0.0.1:49134 HARNESS_E2E_LIVE_NAMESPACE=my-project \
HARNESS_E2E_LIVE_PROVIDER=deepseek HARNESS_E2E_LIVE_MODEL=deepseek-flash \
cargo test --test live_worker -- --ignored
```

Seven scenarios state host paths in their prompts, so their definition digests
match only when the test process has the worker's `HARNESS_E2E_RUN_DIR`,
`TMPDIR`, and `HARNESS_E2E_*_FIXTURE_PATH` values. Export the same environment
the Compose file gives the worker. The scenario run leaves one execution
labelled `live worker validation` in the worker's storage.

### Storage

Durable artifacts are chunked through `storage::*`. Admissions, executions,
runs, attempts, and artifact references are written through the control-plane
`database::*` worker. Execution records keep compact dashboard summaries and
observations, so lists and history do not load native reports.

Storage has no version number and no migration step. Every table records the
fingerprint of the statements that create it. At start, the worker recreates
tables whose fingerprint moved, in one transaction. It keeps the execution
records, local suites and stacks, and receipts it can still read, and rebuilds
run projections from the native bundles. Rows it cannot read, missing bundles,
and tables this binary no longer writes (such as the retired `history_*` import
tables and the `saved_plans` of the retired baseline/candidate plans) are
dropped and logged as warnings. Nothing is reconstructed as a scored result. A
report written under another results contract is read with a warning.

The Console's suites live in `local_suites`, its stacks in `local_stacks`, and
executions (run here or imported from GitHub) in `saved_plan_executions`. A
suite, stack or execution this binary cannot read is deleted on the next read.
An execution a retired plan ran stays, without a suite.

Native bundles keep full reports, manifests, and transcripts, loaded on
demand. The runner has no S3, GCS, R2, SQL-driver, or Harness dependency.

Weekly Stress materializes deterministic fault plans and evaluates journals
from a protected supervisor. Lane promotion is governed by
[`config/policies/cutover.json`](config/policies/cutover.json).

## Observation

The runner waits for a session tree to finish by binding
`harness::turn-completed` to an internal sink, `e2e::on-turn-completed`, before
`harness::send`. That sink is not a control-plane verb: it is not registered
with `e2e::run`, `e2e::status`, or `e2e::cancel`, and it does not appear in
`e2e::scenarios-list`. Subject policies already deny `e2e::*`.

A 15-second watchdog samples `harness::metrics` and one root
`harness::status` for stuck detection, heartbeat logs, and `e2e::cancel`. If
the trigger type is missing from `engine::triggers::list`, the run is
unsupported infrastructure. There is no fallback that polls `harness::status`
or `harness::metrics`. After the tree completes, the runner still collects
terminal status, metrics, transcripts, and deliverables.

From them each run's efficiency measures the prompt the provider processed
(input plus cache reads and writes, since the cache is most of a long
session's prompt), the turns, and the largest prompt one step of the root
session sent, against the window the Harness fit it into. The peak comes from
the usage each step recorded in the transcript, and only when those steps add
up to the session's totals. Compactions are counted from the `compaction`
records the Harness writes to the transcript; the snapshot's `compacted` only
says the session holds a summary now. A run without cache figures reports
them unavailable, never zero. A provider whose billed prompt does not match
the Harness's own count of it (claude-code adds a prompt of its own) is marked
`cache_comparable: false`, and its prompt and cache figures read as not
comparable. The Harness prompt-surface digest of each session role is kept as
data: it changes with every Harness release.

## Evidence

Every completed execution records the subject and E2E revisions, observed wire
contracts, definition digest, materialized inputs, seed, policies, artifacts,
and raw structural evidence.

Deterministic assessment has one payload shape, written only to `results.json`.
Scenario contracts are the only versioned domain. Before cleanup, asset
capture applies explicit safety limits and writes an unversioned sidecar with
the canonical deterministic validation portion, which is aggregated into
`results.json`.

## Repository

| Path | Owns |
| --- | --- |
| `src/` | Runner, wire adapters, scenarios, evaluation, and the E2E control worker. |
| `config/` | The cutover policy, fault profiles, and the master test plan. |
| `stacks/` | Stacks a campaign runs on: iii Compose projects plus the iii release. |
| `tests/` | Test-only fixtures, golden wire schemas, and the Node and Python suites. |
| `schemas/` | Public contracts for generated E2E artifacts. |
| `dashboard/` | React, TypeScript, Vite, and Tailwind Console page embedded in the worker. |

Generated reports, transcripts, logs, and deliverables stay outside Git.

The crate may depend on the iii SDK and on generic libraries. It must not
declare a path or Git dependency on `workers`, Harness, or another product
crate. Contract compatibility is established at runtime from
`engine::functions::list` and `engine::functions::info`. The checked-in schemas
are parity fixtures, not a linked product API.

## Package boundary

This repository executes exact-stack test plans and publishes immutable
`harness-e2e` Registry releases from `main`. A dispatch names a suite, a stack
and a model; the stack is assembled and locked once, and the contract carries
that lock, so a campaign can state afterwards exactly what it ran.

## Releases

`cut-release.yml` is dispatched by hand with `bump` set to `patch`, `minor` or
`major`. It writes the next version into `Cargo.toml` and `Cargo.lock`, commits
that on `main` and pushes the tag `harness-e2e/v<version>`. The tag runs
`release.yml`: it checks that the tag, the manifest and the commit agree, builds
the dashboard once and the binaries for `x86_64-unknown-linux-gnu` and
`aarch64-apple-darwin`, creates the GitHub release, collects the typed
interface in an isolated engine, publishes the Registry candidate to `next`
and promotes it to `latest` with a compare-and-swap on the previous `latest`.
The Registry job runs in the `workers-registry-next` environment; required
reviewers on that environment turn it into a manual approval. Versions are
plain semver; the `-experimental` suffix ended with 0.11.19.

The root `iii.worker.yaml` is the public manifest for local `iii worker`
development and package compatibility. The root `worker-compose.yaml` is a
normal public Compose document. Release Control and post-prepare workflow
phases read neither source contract.
