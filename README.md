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
plus a diagnostic set that suites can opt into.

## Test plans

[config/test-plan.json](config/test-plan.json) is the executable plan. Rust
materializes its suites and execution rules from that source and from the
native scenario contracts. There is no generated catalog to keep in sync.

| Suite | Purpose |
| --- | --- |
| `regression` | Daily runtime, recovery, context and safety checks; one technical retry where safe. |
| `software-engineering` | Kanban, Registry delivery, the trending-topics blog, Linkly, Alertmanager route migration and a playable chess Worker. |
| `pr` | Four essential checks of a candidate stack before merging a change. |
| `after-release` | Five essential checks of the published stack. |

A suite is only what to test: scenarios, runs of each and technical retries.
In the Console these suites are read-only; **Suites** copies any of them into
a suite of the Console to edit, and **Run tests** runs a suite with a model on
the current Harness. See [suites](dashboard/README.md#suites).

```bash
cargo run --locked -- test-plan list
cargo run --locked -- test-plan materialize --suite software-engineering
```

The `software-engineering` suite runs each selected case once, with no
technical retries: the seven Kanban cases, Registry implementation and verification,
the trending-topics build, the Linkly tutorial, Alertmanager route migration,
and the playable chess Worker in the Console. That is 13 cases and 13 planned runs,
in 12 execution groups. Registry
implementation and verification share a group, in that order, so verification
receives the implementation delivery. Trending topics runs in
`case-trending-topics-build`. Linkly runs its eight exchanges in
`case-linkly-tutorial`, using a fresh pinned `linkly-agentic` scaffold as its
Compose project. Baseline worker versions come from the resolved stack contract.
Alertmanager runs in its own group, needs Go 1.25+, and has no turn or token
ceiling.

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

A stack is an iii Compose project plus `iii` (a release, or `latest` for the
newest `iii/v*` release candidate, as Release Control resolves it) and an
optional `template`
(`<id>` or `<id>@<revision>` of `iii-hq/templates`). Credentials never go in a
stack: the executor stamps the namespace, the runner's data directory, the
model's provider, what the suite needs and the private env file per group.
Scripts always come from the dispatched ref.

Preparation resolves the rest, once:

1. `scripts/prepare_execution.py dispatch` reads the inputs into
   `execution.json`, `stack.yaml` and `plan.json`; `runtime` resolves `iii`
   and the template.
2. `runner` resolves the stack's own `harness-e2e` with `iii compose build`,
   pins that release in the stack and fetches it. That binary materializes the
   suite (`suite.json`, also kept as `profile.json`), so the suite always comes
   from the runner every group runs, and the finalizer aggregates with it. The
   runner's identity in the reports is its revision.
3. `contracts` writes one contract per campaign.
4. The stack is assembled once with `compose::add`, which expands every
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
one. A group that produced neither still reports that fact.

`workers` supplies versioned stack components. It does not orchestrate
campaigns.

### Executor image

Every phase runs in one image of tools, `ghcr.io/iii-hq/harness-e2e:tools-<first
12 hex of the Dockerfile's sha256>` ([`Dockerfile`](Dockerfile)): git, curl, jq,
gh, Python 3 with pip and PyYAML, Node 24 with pnpm, Go 1.25, Rust 1.98.1,
Playwright's Chromium at `/usr/bin/chromium` and the Docker CLI with buildx and
compose, much of what the `ubuntu-latest` runner gave the groups before.
Bases, the Ubuntu archive snapshot and every download are pinned, so one
Dockerfile is one set of tools. It holds no scripts:
[`scripts/run_in_image.sh`](scripts/run_in_image.sh) `<phase>` mounts the
checkout at the same path, with a fresh `TMPDIR`, runs as the caller's uid
with `no-new-privileges`, passes the phase's environment through by name, and
runs [`scripts/executor.sh`](scripts/executor.sh) `prepare
[materialize|assemble]`, `group` or `finalize` there. Only `group` gets the
host's Docker socket, and only `prepare` a `GITHUB_TOKEN`: a group's subject
has a shell. Interrupted, the wrapper stops its container; a group whose
image or container never started still writes its `failure.json`.

Each group's engine listens on 49134 in its own container, off the host's
network unless `HARNESS_E2E_DOCKER_NETWORK=host`. The Registry groups need
host networking for their screenshots: the fixture publishes the application
on the host's loopback, where only a phase on the host's network reaches it.
The workflow runs every group on its runner's network, since each job owns
its runner, and keeps on the runner what needs it: checkouts, artifacts, the
OIDC reports, `gh`, and removing a cancelled phase's container before
anything is reported or packaged.

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
unused when a template or agent override is selected. The runner also enables
the campaign's provider when the project does not already have it.

Scenarios, prompts, permissions, fixtures, seeds, and repetitions stay the
same. The evaluated agent applies to ordinary sessions and to workflow or
adaptive steps. Evaluators stay the same. Linkly keeps its pinned task
scaffold and container roles; the selected template's base and agent assets
are applied separately. With no template, the run keeps the existing generated
stack (or the required fixture).

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
assets and the `e2e::dashboard::*` functions for read, suites, run, status, and
cancellation. The page lives under `#/ext/harness-e2e` and exposes Tests,
Executions, and Suites. See [dashboard/README.md](dashboard/README.md).

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
records its parameters with its suite (name and snapshot digest), the stack its
contract names, the workers each group resolved and observed, and its GitHub
origin. Contracts from before the workflow stated its execution (plan and
profile only) import too. A group that left only `failure.json` is kept as a slot with
that error. Importing a run again replaces the runs of the earlier import; the
execution keeps its name. Imported and local executions are the same record:
lists, reports, evidence and renaming treat them alike.

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
records, local suites, and receipts it can still read, and rebuilds run
projections from the native bundles. Rows it cannot read, missing bundles, and
tables this binary no longer writes (such as the retired `history_*` import
tables and the `saved_plans` of the retired baseline/candidate plans) are
dropped and logged as warnings. Nothing is reconstructed as a scored result. A
report written under another results contract is read with a warning.

The Console's suites live in `local_suites`, and executions (run here or
imported from GitHub) in `saved_plan_executions`. A suite or execution this
binary cannot read is deleted on the next read. An execution a retired plan ran
stays, without a suite.

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
