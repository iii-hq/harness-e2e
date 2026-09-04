# Native benchmark tasks

Native tasks are the stable corpus used to measure how Harness capability,
efficiency, convergence and reliability change between system identities. A
task owns its instruction, immutable fixture, execution envelope and
deterministic verifier; it is not a projection of a Rust scenario.

The short-running corpus contains two multi-file bug fixes, one feature, one
code review, one implementation plan, one recovery projection and one recovery
simulation. The existing `engineering_endurance_ladder` is the eighth
capability: it emits a native grounding projection in shadow while retaining
its historical scenario result.

Contracts live under `tasks/*/task.toml`. Adjacent verifier JSON is the
development verifier. An official run loads a runner-private overlay whose
bytes never enter the subject filesystem; the result binds its SHA-256 before
the Harness session starts. Published evidence retains pass/fail, dimension and
output digests while redacting private check ids, commands, values and previews.

## Execution

```text
compile task
  -> verify immutable source
  -> materialize a disposable Git workspace
  -> prove the expected baseline state
  -> execute one Harness turn in that filesystem scope
  -> run deterministic public and private verification
  -> persist task-result.json
  -> teardown the complete Harness session tree
```

Run the offline compiler and fixture validation:

```bash
cargo run --locked -- tasks list
cargo run --locked -- tasks validate
```

Execute one task against a Compose-managed Harness stack:

```bash
cargo run --locked -- tasks run bugfix_config_precedence \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex
```

Execute the complete pilot suite:

```bash
cargo run --locked -- tasks run-suite \
  --suite config/task-suites/pilot.json \
  --model codex/gpt-5.6-luna \
  --provider openai-codex
```

Use `config/task-suites/development-repeatable.json` for a five-sample local
cohort. Remote RC and stable runs use their corresponding suite, an immutable
system manifest and an official verifier bundle:

```bash
cargo run --locked -- tasks run-suite \
  --suite config/task-suites/remote-rc.json \
  --system-manifest /trusted/system.json \
  --official-verifier-bundle /trusted/verifier.json \
  --model codex/gpt-5.6-luna \
  --provider openai-codex
```

Compare two compatible results:

```bash
cargo run --locked -- tasks compare \
  --baseline baseline/task-result.json \
  --candidate candidate/task-result.json
```

Compare cohorts with `tasks compare-suite --baseline ... --candidate ...`.

Recompute a stored cohort from the per-run evidence it already references, so a
suite persisted before a metric existed becomes comparable without re-executing
the model:

```bash
cargo run --locked -- tasks reaggregate \
  --suite-result target/task-runs/<execution>/suite-result.json
```

Comparisons are advisory and emit no deltas when lane, verifier, model, fixture,
runner or another non-Harness component differs.

Local development compares paired executions of the candidate and the last
stable Harness on the same source stack. A published RC compares primarily to
the preceding RC in the same release line and secondarily to the current
stable; a stable compares to the preceding stable. Baseline paths are always
explicit immutable artifacts—there is no mutable implicit `latest` lookup.

## Verifier profiles

- `code_patch` requires a red baseline, exact mutation scope, patch bounds,
  public tests and runner-private probes.
- `structured_artifact` allows only one declared artifact and validates its
  JSON schema plus evidence-bound assertions. It powers review and planning.
- `state_recovery` validates a recovered state and report against the immutable
  initial snapshot, including forbidden-operation evidence.
- `state_simulation` applies the subject's ordered recovery actions to a
  runner-owned state machine and detects stale CAS, replaced identities and
  forbidden direct mutation.

## Longitudinal identity

`case_fingerprint` binds task behavior, fixture manifest, effective verifier,
model, provider, lane and execution limits. `system_identity_sha256` binds the
complete observed system. `cohort_identity_sha256` excludes only the Harness
subject, so a comparison is causal only when Harness is the sole changing
component. Local development and remote release never share a cohort.

Each verifier check has a stable id and one dimension: functional,
structural-integrity, grounding or technical-reliability. Coverage is complete
only when the exact expected check set ran once. Runner/transport failures are
excluded; Harness errors, timeouts and resource limits remain product evidence.

Task outcome keeps `product_passed`, `infrastructure_valid`, `budget_passed`
and `coverage_complete` separate. Tokens, turns, function-call errors and wall
time are retained as independent longitudinal metrics rather than collapsed
into one score.

One included sample is directional, five are repeatable, and twenty are
validated and enable p95. Suites retain Wilson intervals for rates, flakiness,
p50/p95 tokens, turns, function calls, cost and wall time.

Two token series are kept side by side. `total_tokens` counts prompt and
completion only; `billable_tokens` adds the reasoning and cache volume the
provider also moved, which routinely dominates. The two can move in opposite
directions, so an efficiency claim must name which series it means. Monetary
cost is passthrough: a provider that reports no `cost_usd` leaves the series
absent and records the reason in `unavailable`, rather than being imputed from
a price table.

The native task sources live under `native-tasks/` in `iii-hq/e2e-fixture`.
Every task pins the full fixture commit and its subtree manifest independently.
Set `HARNESS_E2E_TASK_FIXTURE_PATH` to an absolute checkout at that exact
revision before validation or execution. Existing scenario fixtures continue
to use `HARNESS_E2E_FIXTURE_PATH`, so the two revision families cannot be mixed
accidentally.
