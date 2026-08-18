# Harness E2E

`harness-e2e` measures which complexity levels a Harness stack can execute
with correct deliverables, structural integrity, bounded work, and repeatable
outcomes.

The repository is intentionally independent from the `workers` source tree.
Runtime discovery, execution, observation, state access, and cleanup all happen
through functions registered in iii. The only product input is an immutable
subject artifact or an already-running iii stack.

## Binaries

- `harness-e2e` starts the iii worker by default and registers the asynchronous
  `e2e::*` control plane plus the injectable Console dashboard. Explicit
  subcommands keep direct scenario execution, report inspection, and the
  standalone dashboard available from the same binary.

Build and validate the repository:

```bash
pnpm --dir dashboard install --frozen-lockfile
pnpm --dir dashboard typecheck
pnpm --dir dashboard lint
pnpm --dir dashboard test
pnpm --dir dashboard build
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
node --test tests/dashboard/*.test.cjs
python3 -m unittest discover -s tests/python -p 'test_*.py'
```

List the materialized scenarios and their scenario versions:

```bash
cargo run --locked --bin harness-e2e -- list
```

Run against an existing stack:

```bash
cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex \
  --scenario coordination.1
```

## Dashboard

Build and start the dashboard from the repository root:

```bash
cargo build --locked --bin harness-e2e
target/debug/harness-e2e dashboard
```

The Rust build follows the same embedded-SPA contract as `workers/console`: it
builds the React bundle with pnpm when `dashboard/dist/` is missing or stale,
then embeds the Vite output in the binary. Node and pnpm must be available on
`PATH`. For frontend development with HMR, use `pnpm --dir dashboard dev`; the
Vite server proxies runtime data, the scoped iii WebSocket, and local-run APIs
to the Rust dashboard on port 4173.

Rust-defined composite scenarios, including the multi-test `security_review`
example, result schema v2 and their common read-only execution projection are documented in
[docs/security-review.md](docs/security-review.md).

The running Harness must publish request and response schemas compatible with
the current typed surface. Missing or incompatible fields fail preflight; no
payload-version compatibility mode is available.

The server listens on `0.0.0.0:4173` by default. Open
`http://localhost:4173/#/overview` on the same machine, or replace `localhost`
with the machine's address when accessing it remotely. Use `--listen
0.0.0.0:PORT` to select another port, `III_URL` to select the running Harness
stack, and `--runs-dir` to select another local history directory.

Local mode loads data incrementally through iii: 25 compact summaries on the
first overview page, one complete report when an execution is opened, only the
selected pair for comparison, and the model/scenario catalog when the run dialog
opens. Server-side filtering and cursor pagination keep history growth out of
the initial payload. Static published and `--view-only` presentations preserve
the generated-file fallback.

Local mode exposes controls that can start and cancel E2E runs, so expose the
port only on a trusted network. Use `--listen 127.0.0.1:4173` when access should
remain local. See [dashboard/README.md](dashboard/README.md) for view-only mode
and the complete dashboard behavior.

## Real control-plane demo

With an iii stack already running, exercise the complete asynchronous
`e2e::*` path using a real coordination scenario:

```bash
HARNESS_E2E_WORKERS_REPOSITORY=iii-hq/workers \
HARNESS_E2E_WORKERS_REVISION=<full-subject-git-sha> \
  ./scripts/demo_e2e_control_plane.sh
```

Use `--catalog-only` for a no-model smoke check. See
[docs/demo.md](docs/demo.md) for the expected flow and retained evidence.

Start the asynchronous worker:

```bash
cargo run --locked --bin harness-e2e -- worker \
  --url ws://127.0.0.1:49134 \
  --data-dir target/e2e-worker
```

The worker uses `~/.iii/data/harness-e2e` by default. Its storage setting is
registered as the `harness-e2e` configuration entry, so it can be changed from
Console → Workers → configure harness-e2e. The YAML passed with `--config` is
only the first-boot seed; a value already saved in Console wins. The command
line `--data-dir` (or `HARNESS_E2E_DATA_DIR`) is an explicit local override and
wins over both. A Console change is applied after restarting the E2E worker;
existing directories are never moved or deleted automatically.

The worker exposes `e2e::run`, `e2e::status`, `e2e::cancel`,
`e2e::results-get`, `e2e::results-list`, `e2e::compare`,
`e2e::scenarios-list`, `e2e::archive`, `e2e::archive-head`,
`e2e::archive-restore`,
`e2e::history-list`, and `e2e::retention-sweep`.
Fault supervisors use `e2e::fault-plan` and `e2e::fault-evaluate` so plan
materialization and recovery classification stay on the same iii control plane.
Subject policies deny `e2e::*`.

Durable artifacts are chunked through `storage::*`, while longitudinal series
are ingested through `database::*`. The runner has no S3, GCS, R2, SQL-driver,
or Harness dependency. See [docs/durable-history.md](docs/durable-history.md)
for retention, deployment, integrity, and recovery details.

Weekly Stress materializes deterministic fault plans and evaluates journals from a
protected supervisor. See [docs/fault-injection.md](docs/fault-injection.md).
Lane promotion is governed by
[`config/policies/cutover.json`](config/policies/cutover.json) and the
[incident/rollback runbook](docs/incident-and-rollback.md).

## Repository boundaries

- `src/` owns the runner, local wire adapters, scenarios, evaluation,
  longitudinal comparison, and the E2E control worker.
- `config/` owns reviewed comparison and cutover policies, fault profiles, and
  standalone stack configuration.
- `tests/` owns test-only fixtures, golden wire schemas, and the Node/Python
  validation suites.
- `schemas/` contains the public contracts for generated E2E artifacts.
- `dashboard/` contains the React, TypeScript, Vite, and Tailwind dashboard
  embedded in the Rust binary.
- generated reports, transcripts, logs, and deliverables stay outside Git.

The crate may depend on the iii SDK and generic libraries. It must not declare
a path or Git dependency on `workers`, Harness, or another product crate.
Contract compatibility is established at runtime from
`engine::functions::list` and `engine::functions::info`; the checked-in schemas
are parity fixtures, not a linked product API.

The assessment and on-demand analysis boundary is documented in
[docs/assessment-result-contract.md](docs/assessment-result-contract.md).
Payloads have one current shape and are written only to `results.json`; scenario
contracts are the only versioned domain.

Deterministic, pre-cleanup asset capture and its explicit safety limits are
documented in [docs/asset-evidence.md](docs/asset-evidence.md). Capture writes an
unversioned sidecar containing the canonical deterministic validation portion,
which is aggregated into `results.json`.

## Observation

The runner waits for a session tree to finish by binding
`harness::turn-completed` to an internal sink (`e2e::on-turn-completed`) before
`harness::send`. That sink is not a control-plane verb: it is not registered
with `e2e::run` / `e2e::status` / `e2e::cancel`, and it does not appear in
`e2e::scenarios-list`. Subject policies already deny `e2e::*`.

A 15s watchdog samples `harness::metrics` and one root `harness::status` for
stuck detection, heartbeat logs, and `e2e::cancel`. If the trigger type is
missing from `engine::triggers::list`, the run is unsupported infrastructure —
there is no silent fallback to polling `harness::status` or `harness::metrics`.
After the tree completes, the runner still collects terminal status, metrics,
transcripts, and deliverables.

## Subject artifacts

Cross-repository executions accept a subject manifest matching
`schemas/subject-artifact.json`. The archive and every declared file are
verified before use. Mutable URLs, shortened Git revisions, unexpected archive
paths, and digest mismatches are rejected.

Untrusted subject artifacts are never given provider, storage, or GitHub
credentials in their environment. Provider workers and the trusted E2E worker
are started separately. PR execution remains non-blocking shadow evidence until
the source repository, revision, E2E ref, and credential boundary are approved.

## Comparison

Every completed execution records the subject and E2E revisions, observed wire
contracts, scenario version, materialized inputs, seed, policies, artifacts,
and raw structural evidence. `e2e::compare` accepts two distinct completed
execution ids (`from_execution_id` and `to_execution_id`) and writes a unique
`comparisons/<comparison-id>/e2e-delta.json` plus `e2e-summary.md`. Numeric
deltas remain disabled when the case set or canonical contract differs.

Deliverable, structural, technical, cost, latency, turns, retries, and work
amplification deltas remain independent. Cost and wall-time are reported as
observed metrics and compared only within a compatible baseline/candidate
cohort.
amplification deltas remain independent. A tier is repeatable after five local
runs satisfy the deliverable, structural, and technical thresholds. Cost and
wall-time are reported as observed metrics and compared only within a compatible
baseline/candidate cohort. See [docs/release.md](docs/release.md) for the
protected environment, immutable identity, artifact, retry, and promotion
boundaries.

## Worker releases

Namespaced stable SemVer tags (`harness-e2e/vX.Y.Z`) build the standard nine
Registry binary targets, create a GitHub prerelease, collect the live typed iii
interface from the released Linux binary, and publish only the Registry
`next` channel.
