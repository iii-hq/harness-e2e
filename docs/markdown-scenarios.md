# Declarative Markdown scenarios

A new declarative E2E scenario requires one file in `scenarios/*.md`. No Rust,
Python, JSON, YAML, or frontmatter change is required.

Use this exact English structure:

```md
# Insert a database record

## Plans

- daily
- weekly

## Version

1

## Before Test

Prepare the isolated database state.

## Prompt

Insert one database record.

## Validations

### Record created (80%)

Confirm that the expected row exists.

### Fewer than 10 turns (20%)

Confirm that the evaluated session used fewer than 10 turns.
```

The file name produces the stable id directly: `insert-record.md` becomes
`insert_record`. Every required H2 section must appear exactly once.
Each validation is an H3 ending in `(N%)`, and all weights must total exactly
100. `Plans` accepts only campaign ids present in `config/campaigns`.

The authored bodies may use two deterministic placeholders: `{{run_id}}` for
an isolated run namespace and `{{seed}}` for the selected case seed. The runner
resolves both before any session starts and freezes the rendered bodies in the
materialized plan. Any other placeholder, or an unclosed placeholder, is a
compile error.

## Local scenarios from Console

Open **Tests** in the Harness E2E Console and choose **New test**. You can start
from the editor template or import a `.md` file. Creating the test only validates
and persists its definition; it does not select the test or start an execution.
Local documents use the same contract above except that `Plans` must contain
only `local`:

```md
## Plans

- local
```

The worker validates the document before saving it under
`<data_dir>/local-scenarios/`. This directory is outside the repository, so
creating a local scenario does not modify Git or require a commit. The new
scenario appears in the local test library and in the Quick run catalog. Open
**Quick run** later when you want to select and execute it. It does not join
daily, weekly, post-release, or reusable baseline plans.

Harness agents can discover the same workflow through iii. Call
`e2e::scenarios-authoring-guide` with `{}` for the copy-ready template and exact
rules, call `e2e::scenarios-create` with `file_name` and `source` to persist the
definition, then call `e2e::scenarios-list` to confirm it has `origin: "local"`.
None of those functions starts a run; `e2e::run` remains a separate, explicit
operation.

When a local scenario starts, the control plane freezes its compiled definition
and exact source inside the execution request. Later file changes therefore do
not alter an admitted or restart-restored run.

Run `cargo run --locked -- validate-scenarios` before committing. On pull
requests, CI also compares the Markdown files with the base revision. A change
to `Before Test`, `Prompt`, validation text, or weights requires a version
increment. Changing only plan participation does not.

At runtime the runner keeps setup, subject, validation, adherence, audit, and
cleanup isolated. It writes the exact source, compiled scenario, immutable
materialized plan (including the rendered bodies), phase transcripts, validator
results, and cleanup evidence under the run's `evidence/` directory. Validation
score, instruction adherence, pipeline integrity, and technical failures remain
separate advisory results.
The adherence analyzer identifies atomic prompt requirements, while the runner
calculates their equal-weight score deterministically. Its evidence distinguishes
failed calls from successful side effects and includes completed validation
outcomes; failed attempts remain visible in the efficiency metrics.

The MVP freezes a bounded tool policy in the materialized plan. Setup may add
workers and prepare database or run-scoped state. The subject may use engine
discovery, worker status, database, and run-scoped state functions; it has no
wildcard access. Validators are read-only. Cleanup receipts must match worker
add/remove, state set/delete, and database table create/drop pairs, otherwise
the run is classified as an infrastructure failure.

Markdown scenarios require an explicit auxiliary model and provider. With the
CLI, pass `--judge-model` and `--judge-provider`; campaigns may instead use the
matching `HARNESS_E2E_JUDGE_MODEL` and `HARNESS_E2E_JUDGE_PROVIDER`
environment variables.

Exact reruns use `harness-e2e replay-materialized <materialized-plan.json>`.
Replay fails before setup if the embedded source, models, tool policies,
budgets, runtime identity, repetition count, or retry policy differs from the
archived plan.
