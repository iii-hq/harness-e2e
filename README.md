# Harness E2E

`harness-e2e` measures which complexity levels a Harness stack can execute
with correct deliverables, structural integrity, bounded work, and repeatable
outcomes.

The repository is intentionally independent from the `workers` source tree.
Runtime discovery, execution, observation, state access, and cleanup all happen
through functions registered in iii. The only product input is an immutable
subject artifact or an already-running iii stack.

## Binaries

- `harness-e2e` runs scenarios directly, reads reports, and serves the local
  dashboard.
- `e2e-worker` registers the asynchronous `e2e::*` control plane in iii.

Build and validate the repository:

```bash
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
node --test dashboard/*.test.cjs
```

List the versioned, materialized scenarios:

```bash
cargo run --locked -- list
```

Run against an existing stack:

```bash
cargo run --locked -- run \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex \
  --scenario coordination.1
```

Start the asynchronous worker:

```bash
cargo run --locked --bin e2e-worker -- \
  --url ws://127.0.0.1:49134 \
  --output-root target/e2e-worker
```

The worker exposes `e2e::run`, `e2e::status`, `e2e::cancel`,
`e2e::results-get`, `e2e::results-list`, `e2e::compare`,
`e2e::scenarios-list`, `e2e::baseline-promote`, `e2e::baseline-get`,
`e2e::archive`, `e2e::archive-head`, `e2e::archive-restore`,
`e2e::history-list`, and `e2e::retention-sweep`.
Fault supervisors use `e2e::fault-plan` and `e2e::fault-evaluate` so plan
materialization and recovery classification stay on the same iii control plane.
Subject policies deny `e2e::*`.

Durable artifacts are chunked through `storage::*`, while longitudinal series
are ingested through `database::*`. The runner has no S3, GCS, R2, SQL-driver,
or Harness dependency. See [docs/durable-history.md](docs/durable-history.md)
for retention, deployment, integrity, and recovery details.

Weekly Stress materializes versioned fault plans and evaluates journals from a
protected supervisor. See [docs/fault-injection.md](docs/fault-injection.md).
Lane promotion and legacy removal are governed by
[`policies/cutover-v1.json`](policies/cutover-v1.json) and the
[incident/rollback runbook](docs/incident-and-rollback.md).

## Repository boundaries

- `src/` owns the runner, local wire adapters, scenarios, evaluation,
  longitudinal comparison, and the E2E control worker.
- `fixtures/`, `golden/`, and `baselines/` contain only small reviewed inputs
  and canonical expectations.
- `dashboard/` contains the static capability dashboard.
- generated reports, transcripts, logs, and deliverables stay outside Git.

The crate may depend on the iii SDK and generic libraries. It must not declare
a path or Git dependency on `workers`, Harness, or another product crate.
Contract compatibility is established at runtime from
`engine::functions::list` and `engine::functions::info`; the checked-in schemas
are parity fixtures, not a linked product API.

## Subject artifacts

Cross-repository executions accept a subject manifest matching
`schemas/subject-artifact-v1.json`. The archive and every declared file are
verified before use. Mutable URLs, shortened Git revisions, unexpected archive
paths, and digest mismatches are rejected.

Untrusted subject artifacts are never given provider, storage, or GitHub
credentials in their environment. Provider workers and the trusted E2E worker
are started separately. PR execution remains non-blocking shadow evidence until
the source repository, revision, E2E ref, and credential boundary are approved.

## Comparison and capability

Every completed execution records the subject and E2E revisions, observed wire
contracts, scenario version, materialized inputs, seed, policies, artifacts,
and raw structural evidence. `e2e::compare` accepts only an immutable promoted
baseline and writes a unique `comparisons/<comparison-id>/e2e-delta.json` plus
`e2e-summary.md`.

Deliverable, structural, technical, cost, latency, turns, retries, and work
amplification deltas remain independent. A tier is not declared reliable until
minimum sample size, quality thresholds, and explicit p95 cost and wall-time
budgets all pass.
