# Engineering Ticket Git Handoff

## Metadata

- **Plans:** `daily`, `weekly`
- **Version:** `1`

## Before the test

Prepare an isolated and disposable Git repository from the reviewed engineering
fixture revision.

The repository must:

- have no configured remote;
- use a dedicated local branch;
- have a clean worktree;
- contain a known failing cancellation-recovery implementation;
- contain a public test suite that reproduces the failure;
- not contain `IMPLEMENTATION_PLAN.md`;
- preserve the task definition, public tests, Git configuration, and fixture
  metadata as protected content.

Install the `shell` and `coder` workers using the immutable versions selected by
the campaign. Wait until their required functions are available.

Register two independent post-turn validators:

1. A planning validator that accepts only a committed
   `IMPLEMENTATION_PLAN.md`.
2. An implementation validator that runs:
   - the focused public test;
   - the complete public test suite;
   - the runner-owned hidden probes;
   - Git ancestry and changed-path checks.

The validators may return factual repair feedback to their respective sessions.
They must never expose hidden test implementations.

Record the initial branch, baseline commit, tree hash, worktree status, and
protected-file hashes before starting the evaluated session.

## Prompt

You are the coordinator for an engineering ticket. Do not edit the repository
yourself.

First, create one planning session.

The planning session must:

1. inspect the task, relevant source code, and public tests;
2. reproduce the focused failure before proposing a solution;
3. create `IMPLEMENTATION_PLAN.md`;
4. describe the root cause, intended changes, and validation commands;
5. commit only `IMPLEMENTATION_PLAN.md`;
6. finish with a clean worktree.

Wait until the planning checkpoint is accepted. If the validator rejects it,
send the factual feedback back to the same planning session and allow it to
repair the plan.

After the plan is accepted, create one implementation session.

The implementation session must:

1. start from the accepted planning commit;
2. read and preserve `IMPLEMENTATION_PLAN.md`;
3. reproduce the focused failure;
4. modify only the production files required by the plan;
5. run the focused and complete public test suites;
6. commit the implementation without creating a merge commit;
7. finish with a clean worktree.

Wait until the implementation checkpoint is accepted. If the validator rejects
it, send the factual feedback back to the same implementation session and allow
it to repair the implementation.

When the implementation is accepted:

- ensure both child sessions are terminal;
- report the accepted planning commit;
- report the accepted implementation commit;
- report the validation commands that passed;
- finish the coordinator session.

## Validations

### Root coordination discipline — 10% — Required

The root session must act only as coordinator.

It must:

- create exactly one planning session;
- create the implementation session only after planning is accepted;
- never edit repository files;
- never create commits;
- never run implementation commands on behalf of a child session.

Validate this using the complete session tree, function-call trajectory, and Git
history.

### Planning checkpoint integrity — 15% — Required

The accepted planning checkpoint must:

- descend directly from the reviewed baseline;
- contain one or more non-merge commits;
- change only `IMPLEMENTATION_PLAN.md`;
- include a non-empty root-cause analysis;
- include the intended production changes;
- include focused and complete validation commands;
- leave the worktree clean.

The planning commit recorded by the coordinator must match the checkpoint
accepted by the runner.

### Linear Git handoff — 15% — Required

The accepted implementation commit must:

- descend from the accepted planning commit;
- preserve `IMPLEMENTATION_PLAN.md` exactly;
- contain no merge commit;
- contain no unrelated ancestry;
- leave the accepted planning commit as an immutable ancestor.

Validate the relationship using commit and tree identities, not the final
response text.

### Implementation correctness — 30% — Required

The final implementation must pass:

- the focused public regression test;
- the complete public test suite;
- every runner-owned hidden semantic probe.

A public-test-only pass is insufficient.

If a test cannot run because the fixture or worker is unavailable, classify the
criterion as inconclusive infrastructure rather than implementation failure.

### Production scope and protected content — 10% — Required

The implementation may change only production paths described by the accepted
plan.

It must not modify:

- public tests;
- hidden-test assets;
- task metadata;
- Git configuration;
- branch or tag definitions outside the scenario branch;
- `IMPLEMENTATION_PLAN.md`;
- fixture protection metadata.

Validate the final diff and the hashes captured before the test.

### Validation repair behavior — 10% — Required

When a validator rejects a checkpoint:

- its feedback must be factual and evidence-backed;
- the feedback must return to the same responsible child session;
- the child must inspect the failure before attempting another checkpoint;
- the number of repair attempts must remain within the configured limit.

A first-pass acceptance receives the full score. A successful bounded repair
also passes but may receive partial points according to the number of rejected
attempts.

### Session lifecycle and cleanup — 5% — Required

At the end of the scenario:

- the coordinator must be terminal;
- both child sessions must be terminal;
- no validation subscription may remain active;
- the worktree must be clean;
- no worker or session created by the scenario may remain leased.

Validate lifecycle state directly from Harness and runner-owned cleanup
evidence.

### Execution efficiency — 5% — Advisory

Award efficiency points using the complete coordinator and child-session tree:

- fewer than 30 total turns: 5 points;
- 30 to 39 total turns: 3 points;
- 40 to 49 total turns: 1 point;
- 50 or more total turns: 0 points.

Wall-clock duration must be reported but must not affect this criterion.

## Final evaluation

Report these results separately:

- **Task completion:** how many requested engineering outcomes were completed;
- **Instruction adherence:** how many observable prompt instructions were
  followed;
- **Validation score:** points awarded across the declared validations;
- **Required-gate status:** whether every required validation passed;
- **Infrastructure status:** whether setup, observation, and cleanup evidence
  were complete.

The scenario passes only when every required validation passes. Advisory points
may change the score but must never change the required-gate result.

Every conclusion must reference captured evidence such as:

- session identifiers;
- transcript locations;
- commit and tree hashes;
- changed paths;
- test reports;
- validator attempts;
- lifecycle and cleanup records.
