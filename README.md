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
- Criterion points stay independent of completion and resource limits.
- Criteria do not veto the score or approve a run.
- Completion, technical validity, artifact evidence, and runtime controls are
  reported separately.
- Infrastructure and execution failures still fail the CLI.

Native criteria keep awards that were already measured when a later check
cannot run. That later check has no award and stays `not_evaluated`. An
incomplete criterion set has no total score. A product failure remains
technically valid. An infrastructure failure invalidates the run and keeps the
observations already recorded.

Every score and every audit flag is deterministic. No scenario calls a second
model to judge the first.

## Requirements

`shell_coder_sandbox`, `chess_engine_build`, and `trend_blog` prepare their
reviewed fixture from an embedded Git bundle. They need Git. They do not need
a fixture checkout or `HARNESS_E2E_FIXTURE_PATH`. Each attempt uses a private
workspace, and temporary source checkouts are removed after their contents are
read.

`chess_engine_build` also needs a Compose daemon and an interactive browser. It
grades a run-scoped chess Worker through live functions and a playable page,
then preserves browser screenshots with the functional evidence. Setup installs
the pinned iii SDK with npm before the run; a cold npm cache needs Registry
access.

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
| `test-plan list` | List profile templates and their coverage. |
| `test-plan materialize` | Expand one profile into campaigns, groups, and cases. |
| `--manifest` | Print the Registry worker manifest as JSON. |

Run one scenario against an existing stack:

```bash
cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex \
  --scenario todo_worker_simple
```

Validate a checked-in campaign without executing it:

```bash
python3 scripts/run_e2e_campaign.py config/campaigns/endurance.json --e2e-bin target/debug/harness-e2e --validate-only
python3 scripts/run_e2e_campaign.py config/campaigns/endurance.json --e2e-bin target/debug/harness-e2e --dry-run
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
- [Linkly tutorial](tests/fixtures/linkly-tutorial/README.md):
  `linkly_tutorial` runs the seven tutorial chapters plus a project-restart
  guard as one scripted dialogue on one Harness session, against the
  `linkly-agentic` scaffold's Compose stack, and scores twenty-two
  deterministic checks. `scripts/linkly_stack.py` prepares that stack.
- [Trending topics](docs/blog-build-contract.md): an isolated per-attempt Git
  remote and an independent Playwright acceptance check against the delivered
  SHA. Design is free. Screenshots are evidence, not an aesthetic score.

`config/test-plan.json` groups the catalog into modules (state, context,
wakes, coordination, software engineering, integration, security, adaptive
operations, continuous engineering, and the incremental Kanban application)
plus a diagnostic set that profiles can opt into.

## Test plans

[config/test-plan.json](config/test-plan.json) is the executable plan. Rust
materializes templates and execution rules from that source and from the
native scenario contracts. There is no generated catalog to keep in sync.

| Profile | Purpose |
| --- | --- |
| `smoke` | Essential behavior on the published stack. |
| `regression` | Representative capabilities, with one technical retry. |
| `capability` | Coverage, repeatability, and difficulty across domains. |
| `evolution` | Quality and resource use across fixed Harness versions. Three repetitions. |
| `endurance` | Sustained correct work and the accepted capability boundary. |
| `software-engineering` | Incremental Kanban, Registry, the trending-topics blog, and the Linkly tutorial. |

In the Console these profiles are starting templates for the same plan form
and the same baseline/candidate view used by saved plans. Choose **New plan**,
optionally pick a template, edit the scope, and select the model. **Save
draft**, **Save and run**, and **Duplicate plan** share one lifecycle and keep
native evidence. See
[executable profile plans](dashboard/README.md#executable-profile-plans).

```bash
cargo run --locked -- test-plan list
cargo run --locked -- test-plan materialize --profile software-engineering
```

The `software-engineering` profile runs each selected case once, with no
technical retries: the seven Kanban cases, four Registry cases, the
trending-topics build, and the Linkly tutorial. That is 13 cases and 13
planned runs, in 12 execution groups. Registry implementation and verification
share a group, in that order, so verification receives the implementation
delivery. Trending topics runs in `case-trending-topics-build`. Linkly runs
its eight exchanges in `case-linkly-tutorial`. The executor creates a fresh
pinned `linkly-agentic` scaffold as that group's Compose project, with
baseline worker versions taken from the resolved stack contract.

## Release Control

Operational campaigns are dispatched only by Release Control, through
[`.github/workflows/exact-stack-e2e.yml`](.github/workflows/exact-stack-e2e.yml).
This repository does not publish independent daily, weekly, post-deploy, or
fault-stress dispatch workflows.

Release Control passes five inputs and makes no further decisions:
`execution_id`, the `plan` (one profile id from `config/test-plan.json`), a
`stack` policy (`{"policy":"latest"}` or exact versions), the executor commit
`runner_sha`, and the `cli_version` to install.

This repository resolves the rest from the commit pinned by `runner_sha`:

1. `harness-e2e test-plan materialize --profile <id>` expands the profile into
   campaigns, groups, and cases, with a `profile_sha256` over the result.
2. `scripts/resolve_stack_lock.py` turns the stack policy into one exact
   `rc-e2e/v2` contract per campaign. Every Registry version is resolved;
   `latest` does not survive into a contract. `scripts/exact_stack_campaign.py`
   validates that contract.
3. Each group runs in an isolated ephemeral stack. Fault groups go to the
   protected runner. One root bundle is produced without rebuilding the native
   Harness artifacts.

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
one. A group that produced neither still reports that fact.

`workers` supplies versioned stack components. It does not orchestrate
campaigns.

### Agent profile and project template

Release Control can name an existing Directory agent profile in the frozen
plan:

```json
{ "agent_profile": "console-ui" }
```

`console-ui` is **Console UI Engineer**. The runner reads that profile from
the group's Directory and sends its id as `agent` to `e2e::run`. The profile,
its parents, skills, functions, and model or provider must exist in that
stack. The runner downloads the stack's versioned skill bundles into an
isolated Directory and waits up to 120 seconds. A profile that is still
missing fails resolution. A profile model overrides the plan model;
`provider::model` also selects its provider. Results record the resolved
subject model and the profile configuration hash. **Run again** resolves the
same profile id in its test stack. Comparisons stay manual in Release Control.
Omitting `agent_profile` keeps the built-in agent.

A project template is independent of the test-plan profile and the agent
profile:

```json
{ "template": "harness", "agent_profile": "tech-lead" }
```

The runner reads `iii/template.yaml` in `iii-hq/templates` from `main`,
resolves it to one commit before sharding, and records that commit as
`identity.template.revision`. Every group uses that source. The selected
Compose project supplies the base. Test-stack versions override its package
selectors, extra packages enter the stack lock, and local workers stay local.
Template skills replace whole downloaded namespaces, and its agent files take
precedence over downloaded profiles. Machine-global profiles and skills are
unused when a template or agent override is selected. The runner also enables
the campaign's provider when the project does not already have it, honoring
any provider version override in the stack contract.

Scenarios, prompts, permissions, fixtures, seeds, and repetitions stay the
same. The evaluated agent applies to ordinary sessions and to workflow or
adaptive steps. Evaluators stay the same. Linkly keeps its pinned task
scaffold and container roles; the selected template's base and agent assets
are applied separately. With no template, the run keeps the existing generated
stack (or the required fixture).

To measure a profile, compare the same plan, template commit, model, and stack
with and without `agent_profile`. Changing the template as well measures the
combined effect. Non-Compose templates and templates that ask for an
interactive language choice are rejected before boot.

## Console

Build the worker and its page from the repository root:

```bash
cargo build --locked --bin harness-e2e
```

When Console connects to the same iii namespace, the worker registers the page
assets and the `e2e::dashboard::*` functions for read, plan, run, status, and
cancellation. The page lives under `#/ext/harness-e2e` and exposes Overview,
Tests, Executions, and Plans. See [dashboard/README.md](dashboard/README.md).

The running Harness must publish request and response schemas compatible with
the current typed surface. Missing or incompatible fields fail preflight.
There is no payload-version compatibility mode.

The page loads through iii: 25 compact summaries on the first overview page,
one complete report when an execution is opened, only the selected pair for
comparison, and the model and scenario catalog when the run dialog opens.
Filtering and cursor pagination run on the server. Transport failures stay
visible. The trusted publisher still writes the bounded JSON report archive
used by CI. It does not publish a standalone Harness E2E web application.

### Compare a local change with Release Control

On Plans, **Reference: Release Control** browses RC history through the
authenticated Release Control browser bridge. Keep the RC tab open, enable its
local Harness connection, and connect it to the same personal Engine as the
Console. The bridge needs the E2E read functions from the companion Release
Control change. No GitHub token or artifact sync is required.

Open a plan to see remote and local executions together, with their origin.
Select a reference and a local result to compare measurements. Scenario links
open the existing A → B comparison with both executions selected. Missing
reports and metrics stay visible as unavailable. Reading history does not
create a local plan, and the comparison sends no local results to Release
Control.

**Run locally** on a remote reference saves that execution's materialized test
parameters as a local plan and runs them against the current Harness.
Repeating the action creates a new local plan from the current scenario
contracts and keeps earlier plans and results. Scenarios, rounds, repetitions,
and retries come from the execution's materialization, not from a current
profile of the same name. The local scenario implementations and Harness are
used on purpose: this is a personal experiment, not an exact-stack
certification. Fault-injection groups still require the protected executor.
References without a shard seed for every scenario cannot be reproduced.
Differences in the local scenario definition or case identity are shown as
advisory information.

Results stay in the local plan store. The RC execution remains a reference.
Native result validation stays strict; remote data is read through the RC API
and is not installed as a native report. Full remote evidence is the
execution's GitHub link, subject to retention. This flow does not download an
evidence archive.

## Worker

Release Control names the exact project roots. This repository writes only the
root configuration and passes `worker@version` references to `compose::add`.
iii resolves the Registry graph, writes the project topology, and reconciles
containers. Every execution starts an empty Engine and a dedicated Compose
daemon, then runs `compose::add`, `compose::up`, `compose::status`, and
`compose::down`. Each execution uses one isolated namespace for Compose and
for the project functions it starts.

Compose supplies `III_URL`, `III_NAMESPACE`, `III_WORKER_NAME`, and
`III_CONFIG`. All four are mandatory. The configuration holds the
execution-specific evidence directory and the separate control-plane database
namespace. Start `worker-compose.control.yaml` before `worker-compose.yaml`.
The control file provisions the single-connection `harness_e2e` SQLite pool
and disables SQL history. The Harness exits when that database or its schema
is unavailable. The subject namespace never receives the database client or
its filesystem path.

Publication validates the locally built binary through a `path://` Compose
container before upload. Published campaigns use only exact Registry package
versions. Provider secrets go to temporary permission-restricted `env_file`
files and never appear in contract, Compose, evidence, or archive artifacts.

The worker exposes:

- `e2e::run`, `e2e::status`, `e2e::cancel`
- `e2e::results-get`, `e2e::results-list`, `e2e::compare`
- `e2e::scenarios-list`
- `e2e::archive`, `e2e::archive-head`, `e2e::archive-restore`
- `e2e::history-list`, `e2e::retention-sweep`

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
records, local plans, and receipts it can still read, and rebuilds run
projections from the native bundles. Rows it cannot read, missing bundles, and
imported Release Control history in a recreated table are logged as warnings.
That history returns by importing it again. Nothing is reconstructed as a
scored result. A report or plan written under another results contract is read
with a warning.

Plan definitions and composed execution receipts live in `saved_plans` and
`saved_plan_executions`. A saved plan or receipt this binary cannot read is
deleted on the next read. Plans written by another binary are not migrated.

Release Control history imports use `harness-e2e-history`, wrapped as
`{json, sha256}` with a `sha256:` digest of the exact UTF-8 JSON. **Import
history** in the Console imports a file or fetches a plan from the RC bridge.
Plans and executions keep source identities, revisions, and every retained
report. Repeated imports do not create duplicates. Imported active work never
enters local admission or recovery. History stays readable without RC.
Evidence uses local `gh` credentials and Python 3 to verify the GitHub bundle
manifest, execution and attempt identity, and file checksums. Missing,
expired, inaccessible, and invalid evidence are separate states. Native
bundles keep full reports, manifests, and transcripts, loaded on demand. The
runner has no S3, GCS, R2, SQL-driver, or Harness dependency.

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

## Comparison

Every completed execution records the subject and E2E revisions, observed wire
contracts, definition digest, materialized inputs, seed, policies, artifacts,
and raw structural evidence. `e2e::compare` takes two distinct completed
execution ids (`from_execution_id` and `to_execution_id`) and writes
`comparisons/<comparison-id>/e2e-delta.json` plus `e2e-summary.md`. Numeric
deltas stay disabled when the case set or the canonical contract differs.

Deliverable, structural, technical, cost, latency, turn, and retry deltas stay
independent. A case is repeatable after five local runs meet the deliverable,
structural, and technical thresholds. Cost and wall time are observed metrics
and are compared only inside a compatible baseline and candidate cohort.

Deterministic assessment has one payload shape, written only to `results.json`.
Scenario contracts are the only versioned domain. Before cleanup, asset
capture applies explicit safety limits and writes an unversioned sidecar with
the canonical deterministic validation portion, which is aggregated into
`results.json`.

## Repository

| Path | Owns |
| --- | --- |
| `src/` | Runner, wire adapters, scenarios, evaluation, longitudinal comparison, and the E2E control worker. |
| `config/` | Comparison and cutover policies, fault profiles, and the master test plan. |
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
`harness-e2e` Registry releases from `main`. Release Control supplies a stack
policy and an immutable executor SHA to `exact-stack-e2e.yml`. The contract
assembled from those inputs pins every Registry version, including historical
candidates, so a campaign can state afterwards exactly what it ran.

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
