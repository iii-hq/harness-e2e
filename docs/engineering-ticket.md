# Daily engineering ticket scenario

> Status: implementation design only. The `engineering_ticket` scenario,
> fixtures, validators, assets, and metrics described here do not exist in this
> repository yet and have not been validated by a live Harness execution.

## Objective

`engineering_ticket` measures whether a normal Harness session can complete a
small but realistic software-maintenance ticket inside an isolated Git
worktree.

Unlike a composite workflow, the benchmark does not orchestrate investigation,
editing, and testing as separate nodes. One root Harness session receives a
natural ticket and owns the complete engineering loop:

```text
receive ticket
  -> inspect repository and instructions
  -> reproduce the failing baseline
  -> diagnose
  -> edit production code
  -> run focused validation
  -> run the full suite
  -> attempt completion
  -> receive factual hidden-test feedback when needed
  -> repair
  -> report observed results
```

The E2E runner prepares and verifies the fixture, observes the session, runs an
independent post-turn auditor, captures assets, evaluates deterministic gates,
and restores the worktree. The subject's final answer and any AI judge remain
non-authoritative.

The initial implementation is a scenario family selected by seed. Every seed
maps to one reviewed `TaskCase` with immutable fixture identity, ticket,
allowed paths, protected paths, public probes, hidden probes, and budgets.

## Why this represents daily Harness usage

The scenario deliberately preserves the normal user experience:

- one ticket in ordinary engineering language;
- one persistent root session with the whole task context;
- unrestricted choice of investigation and implementation strategy inside a
  bounded workspace;
- real code reads, edits, commands, and tests;
- optional subagent use, observed but not required;
- factual feedback when acceptance is incomplete;
- a concise engineering handoff at the end.

It does not require artificial subagent fan-out, a prescribed sequence of tool
calls, or exact source-string matching. Structural gates enforce only behavior
that is materially necessary: relevant inspection, a real red baseline before
the first edit, production-only changes, independent acceptance, and cleanup.

## What the benchmark measures

The result keeps these dimensions separate:

- **task outcome**: the ticket's observable behavior and regression suite pass;
- **engineering discipline**: inspection and reproduction precede mutation,
  tests and contracts remain unchanged, and scope is preserved;
- **self-repair**: the subject understands factual hidden-test feedback and
  converges within a bounded number of validation rounds;
- **deliverable integrity**: patch, manifests, validation results, and handoff
  are captured before cleanup and bound to immutable evidence;
- **efficiency**: turns, tokens, cost, wall time, tool calls, repeated work,
  validation attempts, changed files, and work amplification;
- **runtime reliability**: the Harness session and any children finish and
  teardown without leaked state;
- **benchmark infrastructure**: fixture identity, baseline, tests, validator,
  filesystem boundary, and cleanup are valid.

An AI judge may assess clarity, maintainability, or report usefulness. Those
signals are advisory and cannot promote a failed deterministic outcome.

## Non-goals

- Testing installation of coder, shell, sandbox, or Registry workers. Required
  engineering capabilities are preflight prerequisites.
- Using a real user repository, production credential, network service, or
  mutable remote dependency.
- Requiring a particular implementation or revealing the expected patch.
- Accepting a green public test without hidden semantic acceptance.
- Rewarding changes to tests, fixtures, expected values, or benchmark code.
- Using a judge to decide whether the implementation is correct.
- Combining unlike task cases into one numeric comparison cohort.
- Claiming daily capability from one green execution.
- Automatically opening a pull request, committing, or pushing the candidate
  patch.

## Execution model

The scenario uses the existing `HarnessTurn` path:

```text
ScenarioSpec.setup
  -> harness::send
  -> normal Harness turn loop
  -> post-turn validator zero or more times
  -> terminal observation
  -> deterministic evaluator
  -> deliverable capture
  -> session teardown
  -> ScenarioSpec.cleanup
```

The root session retains context across validator nudges. The evaluator does not
send corrective messages itself and does not reconstruct missing behavior from
the final response.

`technical_retries` may remain available for clean provider or transport
failures before useful work begins, but hard-gate, hidden-test, resource-limit,
or already-started engineering failures are never technically retried. Every
retry keeps a distinct attempt and evidence trail.

## Task case catalog

### Initial cases

The first implementation ships five stable cases. Exact seeds are constants in
Rust and must never be reassigned to a different case.

| Canonical seed | Task case | Work | Initial tier |
| ---: | --- | --- | --- |
| 1001 | `pagination_boundary` | Fix exact-page cursor behavior without breaking empty and partial pages | L2Stateful |
| 1002 | `config_precedence` | Restore `environment > file > default` precedence across two modules | L2Stateful |
| 1003 | `serialization_compatibility` | Add an optional field while preserving old payload decoding and omission behavior | L3Concurrent |
| 1004 | `cache_invalidation` | Prevent stale derived state after a write crosses repository and service layers | L3Concurrent |
| 1005 | `async_cancellation` | Guarantee resource cleanup and terminal state after cancellation | L4Coordinated |

Arbitrary seeds map deterministically to one catalog entry but retain their own
`case_id`. Operational daily workflows use only reviewed named seed constants.
Adding or remapping catalog entries, changing fixture bytes, prompts, probes,
gates, or budgets requires a scenario-version review.

### `TaskCase` contract

Add a Rust-owned `TaskCase` definition containing:

```text
id
case_version
canonical_seed
fixture_repository
fixture_revision
fixture_manifest_sha256
ticket
focused_test_command
full_test_command
allowed_production_paths
protected_paths
relevant_read_paths
public_probe_ids
hidden_probe_manifest_sha256
maximum_validation_rounds
maximum_changed_files
maximum_patch_lines
complexity_profile
```

The ticket and public commands are visible to the subject. Hidden probe bodies,
expected patch, root-cause reference, and evaluator implementation are not.

`ScenarioCase.inputs` persists all non-secret fields needed to identify and
reproduce the case, including `task_case_id`, `case_version`, fixture and hidden
manifest hashes, policies, commands, and budgets. Comparison remains eligible
only for the same scenario version, seed, inputs hash, and contract fingerprint.

## Fixture boundary

### Prepared worktree

The environment prepares one disposable Git worktree for the selected seed and
exposes it through:

```text
HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH
```

The fixture path is execution-specific. Anchor and rotating cases run as
separate executions with separate worktrees. A single execution must not use
one fixture path for multiple task seeds.

The worktree contains:

- `.git` metadata pointing only to the disposable fixture repository;
- `.harness-e2e/task-case.json` with task id, seed, revisions, commands,
  policies, and exact hashes;
- source code at the seeded broken revision;
- public tests available to the subject;
- hidden tests outside the subject filesystem scope;
- a filesystem-scoped shell execution profile with outbound networking disabled
  by the protected launcher;
- a clean initial Git status;
- no credentials, mutable network dependency, or external write target.

The scenario sets `ScenarioSpec.filesystem_root` to the canonical fixture path,
which causes the runner to pass the existing `fs_scope` metadata to Harness.
`scenario_for_case` reads the environment without mutating the filesystem: it
uses `None` when the variable is absent during catalog materialization and the
case-specific setup rejects an actual execution without the required path.

### Fixture preparation

Fixture creation remains outside the subject session. A reviewed helper script
or protected daily launcher:

1. resolves the requested `TaskCase` from the seed;
2. checks out the exact fixture revision into a new worktree;
3. materializes the public case manifest;
4. places hidden probes outside `fs_scope`;
5. runs fixture self-tests;
6. verifies the declared red baseline;
7. applies and records the network-disabled shell profile identity;
8. exports the absolute fixture path;
9. invokes the E2E runner;
10. retains launcher logs as infrastructure evidence.

The runner independently verifies the same identities. A successful launcher
does not replace scenario preflight.

### Preflight rules

The case-specific setup function rejects:

- missing, relative, non-canonical, or non-UTF-8 fixture paths;
- symlinks escaping the worktree;
- missing or unexpected Git repository identity;
- dirty initial state;
- mismatch between selected seed and task manifest;
- fixture revision or manifest hash mismatch;
- commands outside the allowlisted command contract;
- public baseline that does not produce the expected failing probe;
- unrelated public failures;
- missing coder, shell, trigger, or Harness hook capabilities;
- missing or mismatched network-isolation profile identity;
- known or shape-detected secrets in fixture content or environment evidence.

Because `ScenarioSetup` receives only `run_id`, `scenario_for_case` selects a
case-specific setup function pointer:

```text
TaskCase::PaginationBoundary -> setup_pagination_boundary
TaskCase::ConfigPrecedence -> setup_config_precedence
...
```

Each wrapper calls a shared setup implementation with its static expected case.
This supports seed-specific fixtures without changing the runner hook contract.

## Scenario contract

Add a module at:

```text
src/scenarios/engineering_ticket/
  mod.rs
  cases.rs
  fixture.rs
  prompt.rs
  auditor.rs
  evaluate.rs
  evidence.rs
  metrics.rs
  tests.rs
```

Register one scenario id:

```text
id: engineering_ticket
scenario_version: 1
execution_kind: harness_turn
manual_cli_only: true initially
canonical seed: 1001
```

`materialize(namespace, seed)`:

1. resolves the reviewed `TaskCase`;
2. builds the seed-specific natural prompt;
3. selects the case-specific setup function;
4. uses the common cleanup and evaluator;
5. materializes complexity, capabilities, work expectation, and deliverable
   contract from the case;
6. validates that the fixture environment path matches the selected case at
   execution preflight, never during pure catalog listing.

Required capabilities are:

```text
e2e::control-plane-v1
iii::functions
iii::coder
iii::shell
iii::triggers
harness::post-turn-validation
```

Sandbox and subagent capabilities are optional case metadata. Their absence
cannot fail a case that does not require them.

### Execution policy

Initial case defaults are:

```text
max_turns: 48
max_output_tokens: 16384
max_total_tokens: 600000
stuck_timeout_seconds: 600
maximum_validation_rounds: 2
```

Cases may lower these budgets but cannot increase them without a
scenario-version review. Cost is measured from observed subject usage and
evaluated longitudinally; absence of provider cost data remains unavailable,
not zero.

## Natural subject prompt

The prompt has four short sections:

1. ticket title and user-visible problem;
2. workspace and public command contract;
3. safety boundaries;
4. requested handoff.

Example shape:

```text
You are assigned engineering ticket ET-1001 in the current repository.

Users report that pagination omits the final item when the total number of
items is an exact multiple of page size. Investigate the repository, reproduce
the reported behavior, implement the smallest sufficient production change,
and validate it.

You may modify only the declared production paths. Do not modify tests,
fixtures, task metadata, Git metadata, or expected values. Run the focused test
and the full suite before reporting completion. Report the files changed,
commands run, observed results, and any remaining limitation.
```

The prompt does not prescribe a file, condition, implementation, tool order,
number of edits, or subagent strategy. Validator messages are explicitly
identified as trusted Harness post-turn validation feedback, but their future
contents are not disclosed.

## Post-turn auditor

### Registration

Setup registers one attempt-owned function and one scoped
`harness::hook::post-turn` subscription before `harness::send`.

The function id and result paths include a sanitized run suffix. Registration
is restricted to the E2E-owned root session. Foreign-session registration and
subject access to the auditor function are denied.

### Evaluation order

On every completion attempt, the auditor:

1. verifies fixture and protected-file integrity;
2. verifies a production patch exists;
3. runs the focused public probe in a bounded subprocess;
4. runs hidden semantic probes outside subject scope;
5. runs the full public suite;
6. measures patch and scope budgets;
7. persists an attempt result before returning its verdict.

The subject cannot observe hidden source, expected patch, or raw evaluator
implementation. It receives only bounded factual failures.

### Feedback contract

Example feedback:

```text
VALIDATOR: acceptance is incomplete.
- hidden probe empty_last_page: page_size=10, total=20, cursor=20 returned
  1 item; expected 0.
- protected files changed: none.
- focused public test: passed.
Repair the production implementation, rerun relevant tests, and report the
observed result. The validator will not prescribe the patch.
```

The auditor must not:

- name the expected code change;
- reveal hidden test source;
- recommend a specific algorithm;
- rewrite the ticket;
- continue after the validation-round budget;
- convert evaluator infrastructure failure into subject feedback.

Infrastructure failures return a technical error and preserve partial evidence.
They do not consume a subject repair round.

### Acceptance

The final attempt is accepted only when:

- fixture and protected paths remain exact;
- the focused probe passes;
- every hidden semantic probe passes;
- the full public suite passes;
- change and validation budgets pass;
- no prohibited external effect is observed.

Acceptance is silent to the subject. The runner later evaluates the transcript
and durable attempt results independently.

## Deterministic evaluator

The final evaluator combines:

- preflight and red-baseline records;
- ordered subject function calls and results;
- Git diff and status;
- validator attempt records;
- focused and full-suite outputs;
- hidden probe summaries;
- session metrics and child-session tree;
- final response;
- cleanup reconciliation.

It never infers a missing baseline, test run, or file read from the final prose.

### Engineering-discipline gates

- `fixture_identity_exact`
- `fixture_clean_before_run`
- `red_baseline_verified_by_runner`
- `relevant_source_read_before_first_edit`
- `relevant_test_read_before_first_edit`
- `subject_reproduced_failure_before_first_edit`
- `production_patch_present`
- `allowed_paths_only`
- `tests_unchanged`
- `task_manifest_unchanged`
- `git_metadata_unchanged`
- `no_network_or_external_write`

Relevant reads are expressed as reviewed path classes, not brittle exact source
strings. Equivalent normalized relative, `./`, and absolute paths resolving
inside the fixture root count as the same file. Traversal and outside-root paths
never count.

### Acceptance gates

- `focused_test_passed`
- `hidden_semantic_cases_passed`
- `full_suite_passed`
- `original_failure_eliminated`
- `unrelated_behavior_preserved`
- `patch_file_budget_passed`
- `patch_line_budget_passed`

### Loop gates

- `completion_attempt_observed`
- `validator_attempts_persisted`
- `validator_feedback_factual`
- `validation_round_budget_respected`
- `accepted_after_latest_patch`

A first-pass success is valid and produces zero nudges. A case may require at
least one hidden repair only when that requirement is part of the reviewed case
contract; the general daily scenario must not fail an immediately correct
solution merely to demonstrate self-repair.

### Lifecycle gates

- `root_session_terminal`
- `child_sessions_terminal`
- `owned_validator_retired`
- `assets_captured_before_cleanup`
- `fixture_restored_after_cleanup`
- `no_owned_temporary_resource`

## Assessments

The initial deterministic assessment weights total 100:

| Assessment | Weight | Policy | Dimension |
| --- | ---: | --- | --- |
| `engineering_discipline` | 25 | hard gate | structural integrity |
| `ticket_acceptance` | 40 | hard gate | deliverable |
| `validation_convergence` | 20 | hard gate | structural integrity |
| `scope_and_lifecycle` | 15 | hard gate | structural integrity |

Optional advisory signals include:

- diagnosis clarity;
- implementation maintainability;
- test-selection quality;
- final handoff usefulness;
- residual-risk awareness.

Partial advisory points never compensate for a failed hard gate.

## Assets

All assets are captured after the session becomes terminal and before teardown
or fixture cleanup. Content is bounded, redacted, schema-validated, hashed, and
bound to provenance.

| Asset id | Kind | Producer | Required content |
| --- | --- | --- | --- |
| `ticket_contract` | `engineering_ticket_contract` | setup | task identity, seed, fixture, commands, policies, budgets |
| `baseline_record` | `engineering_baseline` | setup | command, exit, expected failure, probe ids, fixture hashes |
| `inspection_record` | `engineering_inspection` | evaluator | relevant reads, first edit, first test, ordering evidence |
| `candidate_patch` | `code_patch` | capture | bounded diff against the initial revision |
| `change_manifest` | `change_manifest` | evaluator | before/after hashes, changed paths, line counts, protected-path result |
| `validation_matrix` | `validation_matrix` | auditor/evaluator | every attempt's focused, hidden, and full-suite outcomes |
| `repair_timeline` | `repair_timeline` | evaluator | completion attempts, factual nudges, patch hashes, convergence |
| `engineering_report` | `engineering_report` | capture | structured final response and exact claimed commands/results |

The deliverable contract declares eight artifacts and the matching complexity
profile uses `artifact_count = 8`.

Raw hidden test source is never captured. `validation_matrix` retains stable
probe ids, result status, bounded factual observations, duration, and output
hashes.

The runner separately retains transcript, session metrics, cost, failures,
assessment results, asset-capture manifest, cleanup reconciliation, final AI
assessment input, and canonical `results.json`.

## Normalized metrics

### Already available from the runner

- wall time;
- root and child turns;
- root and child sessions;
- function calls and function-call errors;
- input, output, and total tokens;
- provider-reported cost;
- validation retries;
- technical attempts;
- work amplification;
- run and failure status.

### Derived from transcript and assets

- call index of first relevant source read;
- call index of baseline reproduction;
- call index of first edit;
- calls between first edit and first focused green;
- focused and full-suite command counts;
- identical command repetitions;
- files and lines changed;
- protected-path violations;
- completion attempts;
- hidden validation nudges;
- first-pass acceptance;
- repair convergence;
- patch growth after each nudge;
- optional subagent count and contribution.

Time-to-read, time-to-reproduction, or time-to-first-edit is reported only when
the transcript provides trustworthy event timestamps. Otherwise the ordered
call index remains available and time stays explicitly unavailable.

### Per-case longitudinal metrics

- `task_success_rate`;
- `engineering_discipline_rate`;
- `first_pass_acceptance_rate`;
- `repair_convergence_rate`;
- `scope_preservation_rate`;
- `technical_failure_rate`;
- `flaky_rate`;
- median validation rounds;
- p50/p95 wall time;
- p50/p95 cost;
- p50/p95 total turns;
- p50/p95 work amplification;
- tool-error rate;
- median changed files and patch lines.

Numeric comparison is disabled when case identity, fixture, scenario contract,
subject/judge identity, lane, or stack mode is incompatible. Infrastructure
runs are excluded from subject capability rates and retained in the cohort
audit.

## Complexity profiles

Every `TaskCase` has its own materialized profile. Suggested starting values:

| Dimension | L2 case | L3 case | L4 case |
| --- | ---: | ---: | ---: |
| planning depth | 2 | 3 | 4 |
| dependency depth | 1 | 2 | 3 |
| parallel branches | 1 | 2 | 2 |
| external systems | 2 | 2 | 3 |
| state transitions | 5 | 7 | 10 |
| wake cycles | 0 | 0 | 1 |
| validation loops | 2 | 2 | 2 |
| artifact count | 8 | 8 | 8 |
| coordination edges | 1 | 2 | 5 |
| ambiguity level | 2 | 4 | 6 |

`minimum_expected_work` may be overridden per case after observing a validated
pilot. It is never tuned using a single favorable model run.

## Cleanup

Cleanup is case-independent because every execution receives one exact fixture
path through the environment.

It attempts, in order:

1. retire the attempt-owned post-turn subscription;
2. unregister the attempt-owned auditor function where supported;
3. stop and teardown remaining root or child sessions;
4. persist final validator and Git state;
5. reset the worktree to the exact initial fixture revision;
6. remove untracked attempt files and temporary branches only inside the
   validated fixture root;
7. verify clean status, exact HEAD, no owned subscription, and no active child.

Cleanup does not delete captured E2E assets. It must avoid a broad recursive
delete and reject an empty, root, home, or unresolved fixture path.

A correct patch with failed cleanup does not pass the scenario.

## Failure classification

| Condition | Classification |
| --- | --- |
| Wrong behavior, hidden probe failure, test modification, scope violation, or lifecycle hard gate | hard-gate failure |
| Harness cannot complete the task despite available infrastructure | subject error |
| Turn, token, stuck, validation-round, or other declared resource budget exceeded | resource limit |
| Judge unavailable, malformed, or timed out | advisory judge state |
| Fixture mismatch, baseline self-test failure, auditor crash, missing capability, or cleanup mechanism failure | infrastructure error |

`E2E suite failed` remains an aggregate message. Per-case status, hard gates,
validator evidence, failure phase, and infrastructure health are authoritative.

## Daily operation

Anchor and exploratory executions are intentionally separate because `--runs`
currently applies the same repetition count to the primary and rotating seeds.

### Fixed daily anchor

Run the same canonical case five times:

```bash
HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH=/absolute/path/to/anchor-fixture \
  cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model <subject-model> \
  --provider <subject-provider> \
  --scenario engineering_ticket \
  --seed 1001 \
  --runs 5 \
  --runs-dir target/engineering-ticket-daily
```

The lane, subject, judge, stack mode, exact revisions, fixture contract, and E2E
revision remain fixed within the comparison cohort. Five runs provide
repeatable evidence for reliability and flakiness; p95 claims remain immature.

### Rotating daily ticket

Run one reviewed alternate case as directional evidence:

```bash
HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH=/absolute/path/to/rotating-fixture \
  cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model <subject-model> \
  --provider <subject-provider> \
  --scenario engineering_ticket \
  --seed 1002 \
  --runs 1 \
  --runs-dir target/engineering-ticket-daily
```

The rotating result is not numerically compared with seed 1001. Once a case is
useful and stable, run its exact seed five times and promote it into the weekly
pack. Reserve 20-run baseline/candidate cohorts for p95 validation after both
the case and fixture have live evidence.

### Daily decisions

Report these decisions independently:

```text
product_passed
engineering_discipline_passed
hidden_coverage_passed
infrastructure_valid
budget_passed
cleanup_passed
daily_anchor_healthy
```

No aggregate score may make `daily_anchor_healthy` true while another required
decision is false.

## Code registration changes

Implementation requires:

1. Add `pub mod engineering_ticket` in `src/scenarios/mod.rs`.
2. Add `ScenarioId::EngineeringTicket` and register id, spec, materialization,
   the explicit canonical seed `1001`, and `HarnessTurn` execution kind.
3. Mark the scenario manual-only until fixture preparation and daily launcher
   adoption are validated.
4. Add the module files listed above without adding a separate runner.
5. Reuse `ScenarioSpec.filesystem_root`, setup, evaluator, capture, and cleanup
   hooks.
6. Reuse the current results and asset contracts; do not add another payload
   version or dashboard endpoint.
7. Keep dashboard work generic: the existing assessment, asset, transcript,
   efficiency, and longitudinal projections should display this scenario.
8. Add a launcher or workflow integration only after local live validation and
   explicit adoption approval.

The catalog list path must remain side-effect free. It may materialize case
metadata but must not create a worktree, run tests, or require the fixture env
variable until an execution begins.

## Test plan

### Task case and materialization tests

- Canonical seeds map to stable task ids and inputs hashes.
- Arbitrary-seed mapping is deterministic.
- Seed remapping or fixture hash changes alter the materialized contract.
- Each task case validates positive budgets, path policies, commands, and
  hidden manifest identity.
- All cases produce their expected complexity tier and eight-asset contract.
- Catalog listing remains pure without fixture environment variables.

### Fixture and path tests

- Preflight accepts only the exact clean fixture and selected task manifest.
- Relative, traversal, outside-root, symlink-escape, home, and root paths fail.
- Equivalent relative, `./`, and absolute in-root paths normalize identically.
- Wrong seed, revision, manifest, protected hash, or baseline fails as
  infrastructure.
- Cleanup restores byte-identical tracked content and removes only owned
  untracked files.

### Evaluator tests

- Relevant source and test reads must precede the first successful edit.
- A green final workspace without an observed subject baseline fails the
  discipline gate.
- Public green with hidden red fails acceptance.
- Hidden green with modified tests or manifest fails integrity.
- Equivalent valid implementations pass without matching an expected diff.
- Patch/file budgets and changed-path policy are exact.
- Function-call results are correlated by call id and function id.
- Missing metrics remain unavailable rather than zero.

### Auditor tests

- First-pass correct patch returns acceptance without a nudge.
- A public-only patch receives factual hidden feedback and can converge.
- Feedback never includes hidden source or a prescribed repair.
- Maximum validation rounds is enforced.
- Auditor infrastructure failure is not counted as a subject repair.
- Attempt records are persisted before the verdict.
- Stale patch acceptance is rejected when the workspace changes afterward.

### Lifecycle tests

- Setup registers exactly one owned validator.
- Teardown and cleanup retire all owned resources.
- Cancellation during inspection, edit, test, and validator execution cleans
  partial state.
- Subject, resource, evaluator, and cleanup failures retain distinct phases.
- Assets remain readable and hash-valid after workspace restoration.

### Live validation

Validate in increasing scope:

1. fixture self-tests;
2. Rust scenario/unit tests;
3. focused runner tests;
4. one catalog/preflight check;
5. one real Harness run of seed 1001;
6. one intentionally insufficient first patch proving factual self-repair;
7. five-run anchor repetition;
8. dashboard inspection of results, assets, transcript, deltas, and cleanup.

Compile, unit-test, or synthetic executor evidence alone is insufficient to
describe the daily benchmark as operationally adopted.

## Implementation sequence

1. Freeze the five `TaskCase` contracts, fixture repository, commands, hidden
   manifests, and budgets.
2. Implement and self-test the protected fixture preparer.
3. Add case resolution, scenario materialization, and side-effect-free catalog
   registration.
4. Add case-specific setup wrappers and shared preflight.
5. Add the natural prompt, post-turn auditor, and bounded factual feedback.
6. Add deterministic evaluator, metric derivation, assets, and cleanup.
7. Complete unit, fixture, evaluator, auditor, cancellation, and schema tests.
8. Execute seed 1001 against a real local Harness and inspect retained evidence.
9. Repeat the anchor five times and establish the first compatible baseline.
10. Only then connect it to a daily lane and begin rotating reviewed cases.

## Definition of done

The scenario is implemented only when:

- every case has immutable fixture, prompt, probe, path, and budget identity;
- one natural Harness session owns the entire ticket lifecycle;
- the subject proves a real red baseline before its first edit;
- correctness is decided by independent public, hidden, and regression probes;
- an immediately correct patch passes without an artificial required nudge;
- an incomplete patch can receive factual feedback and repair within budget;
- tests, fixtures, contracts, and out-of-scope paths cannot be modified to pass;
- all eight assets are captured, redacted, schema-valid, and hash-bound before
  cleanup;
- cancellation and every failure phase prove fixture and session cleanup;
- at least one live execution and one five-run anchor cohort are inspectable;
- quality or AI analysis remains advisory;
- no user repository, production system, external write target, commit, push,
  or pull request is touched.
