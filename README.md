# Harness E2E

`harness-e2e` measures which complexity levels a Harness stack can execute
with correct deliverables, structural integrity, bounded work, and repeatable
outcomes.

The repository is intentionally independent from the `workers` source tree.
Runtime discovery, execution, observation, state access, and cleanup all happen
through functions registered in iii. The only product input is an immutable
subject artifact or an already-running iii stack.

The SWE service suite provides eight isolated engineering
tasks and a continuous eight-ticket journey over the same Python service, with
optional delegation, immutable checkpoints, isolated verification, and a trusted
GitHub handoff.

## Binaries

- `harness-e2e` is started by Compose and registers the asynchronous `e2e::*`
  control plane plus the injectable Console dashboard. Explicit
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
HARNESS_E2E_BIN="$PWD/target/debug/harness-e2e" python3 -m unittest discover -s tests/python -p 'test_*.py'
```

List the materialized scenarios and their scenario versions:

```bash
cargo run --locked --bin harness-e2e -- list
cargo run --locked --bin harness-e2e -- catalog
cargo run --locked --bin harness-e2e -- validate-scenarios
```

New declarative scenarios are authored only as `scenarios/*.md`. The compiler
embeds the exact source, validates the canonical English section structure,
and exposes the resulting file-stem id through the CLI, worker catalog,
campaign runner, dashboard, and canonical result artifacts. Required sections are
Version, Before Test, Prompt and Validations. Plans select their scenarios explicitly.

Replay an archived input only through its immutable plan (the runner rejects
any scenario, model, policy, budget, stack, runner, run-count, or retry drift):

```bash
cargo run --locked -- replay-materialized \
  target/e2e/evidence/<run-id>/<attempt-id>/materialized-plan.json
```

Run against an existing stack:

```bash
cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex \
  --scenario todo_worker_simple
```

Validate one of the checked-in canonical campaign assets:

```bash
python3 scripts/run_e2e_campaign.py config/campaigns/endurance.json --e2e-bin target/debug/harness-e2e --validate-only
python3 scripts/run_e2e_campaign.py config/campaigns/endurance.json --e2e-bin target/debug/harness-e2e --dry-run
```

Operational campaign execution is dispatched only by Release Control through
`.github/workflows/exact-stack-e2e.yml`. The repository no longer
publishes independent daily, weekly, post-deploy, or fault-stress dispatch
workflows.

Campaign manifests never select or rotate seeds. They separate replay-safe
turns from scripted dialogue and composite flows, persist a summary for every
group, and are advisory by default while their longitudinal history is being
calibrated. Release Control owns scheduling and dispatch; the executor keeps
the result advisory and archives each materialized group through the
environment-owned durable archiver.
The code-focused campaigns use protected disposable checkouts of
`iii-hq/e2e-fixture`. The engineering handoff uses its dedicated pinned
revision, while `shell_coder_sandbox`, `chess_engine_build`, and `trend_blog`
share a second pinned revision through `HARNESS_E2E_FIXTURE_PATH`. The protected
launcher enforces the fixture and cleanup boundary.
`typescript_chat_service` carries its own frozen skeleton in the repository and
needs no checkout, but it does require Node 22.6 or newer on the runner host: the
subject's TypeScript application is executed directly through Node type
stripping, both by the public suite and by the runner-owned behavioral probe.
[config/test-plan.json](config/test-plan.json) defines the six executable profiles: smoke, regression, capability, evolution,
resilience, and endurance. In the dashboard these profiles are starting templates
for the same plan form and baseline/candidate visualization used by existing plans.
Choose **New plan**, optionally select a template, edit the scope, and select the
execution and judge models. **Save draft**, **Save and run**, and **Duplicate plan**
use one shared lifecycle and retain native evidence. Fault-injection plans export
to the protected executor. See [executable profile plans](dashboard/README.md#executable-profile-plans).

```bash
cargo run --locked -- test-plan list
```

Templates and execution rules are materialized directly by Rust from the source
and native contracts. There are no generated catalogs to synchronize.

Release Control dispatches `.github/workflows/exact-stack-e2e.yml` directly in
this repository with five inputs and no decisions of its own: `execution_id`,
the `plan` naming one profile of `config/test-plan.json`, a `stack` policy
(`{"policy":"latest"}` or exact versions), the executor commit `runner_sha`,
and the `cli_version` to install.

Everything else is resolved here, from the commit pinned by `runner_sha`:

1. `harness-e2e test-plan materialize --profile <id>` expands the profile into
   its campaigns, groups and cases, with a `profile_sha256` over the result.
2. `scripts/resolve_stack_lock.py` turns the stack policy into one exact
   `rc-e2e/v2` contract per campaign — every Registry version resolved, `latest`
   never surviving into a contract — which `scripts/exact_stack_campaign.py`
   validates as before.
3. Each group runs in an isolated ephemeral stack; fault groups route to the
   protected runner; one root bundle is produced without rebuilding the native
   Harness artifacts.

`scripts/report_execution.py` posts what was observed to Release Control's run
ledger over OIDC: `materialized` before anything runs, one `shard` per campaign
group whatever that group did, and a `summary` whatever the finalizer did. Runs
come from `results.json`, or from the journal checkpoints when a group died
before writing one; a group that produced neither still reports, saying so. No
execution is silently lost.

`workers` supplies versioned components of the stack under test; it does not
orchestrate campaigns.

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
example, use the current shared result schema and read-only execution projection.

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

## Compose lifecycle

Release Control names the exact project roots. This repository writes only the
root configuration and passes those `worker@version` references to
`compose::add`; iii resolves the Registry graph, writes the project topology,
and reconciles its containers. Every execution starts an empty Engine and a
dedicated Compose daemon, then runs `compose::add`, `compose::up`,
`compose::status`, and `compose::down`. Each execution uses one isolated
namespace for both Compose and the project functions it starts.

Compose supplies `III_URL`, `III_NAMESPACE`, `III_WORKER_NAME`, and `III_CONFIG`
to the `harness-e2e` process. All four values are mandatory. The referenced
configuration file contains the execution-specific `data_dir`; there is no
local fallback, command-line override, or runtime self-registration.

Publication validates the locally built binary through a `path://` Compose
container before the package is uploaded. Published campaigns use only exact
Registry package versions. Provider secrets are written to temporary
permission-restricted `env_file` files and are never included in contract,
Compose, evidence, or archive artifacts.

The worker exposes `e2e::run`, `e2e::status`, `e2e::cancel`,
`e2e::results-get`, `e2e::results-list`, `e2e::compare`,
`e2e::scenarios-list`, `e2e::scenarios-create`,
`e2e::scenarios-authoring-guide`, `e2e::archive`, `e2e::archive-head`,
`e2e::archive-restore`,
`e2e::history-list`, and `e2e::retention-sweep`.
Fault supervisors use `e2e::fault-plan` and `e2e::fault-evaluate` so plan
materialization and recovery classification stay on the same iii control plane.
Subject policies deny `e2e::*`.

Durable artifacts are chunked through `storage::*`, while longitudinal series
are ingested through `database::*`. The runner has no S3, GCS, R2, SQL-driver,
or Harness dependency.

Weekly Stress materializes deterministic fault plans and evaluates journals from a
protected supervisor.
Lane promotion is governed by
[`config/policies/cutover.json`](config/policies/cutover.json).

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

The assessment and on-demand analysis boundary has one current payload shape,
written only to `results.json`; scenario contracts are the only versioned
domain.

Deterministic, pre-cleanup asset capture applies explicit safety limits and
writes an unversioned sidecar containing the canonical deterministic validation
portion, which is aggregated into `results.json`.

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
baseline/candidate cohort.

## Runtime-only package boundary

This repository executes exact-stack Test Plans and never publishes itself as
a Registry worker. Release Control supplies a stack policy and an immutable
executor SHA to `exact-stack-e2e.yml`; the contract this repository assembles
from them pins every Registry version to an exact one, including historical
candidates, because a campaign has to be able to say afterwards what it ran.

The root `iii.worker.yaml` remains the public manifest for local `iii worker`
development and package compatibility. The root `worker-compose.yaml` remains
a normal public Compose document. Release Control and post-prepare workflow
phases deliberately read neither source contract.
