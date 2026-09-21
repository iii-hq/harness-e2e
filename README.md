# Harness E2E

`harness-e2e` measures what a Harness stack can execute with correct
deliverables, structural integrity, bounded work, and repeatable outcomes.

A run's score is the plain sum of the points its evaluated criteria awarded; a
criterion nobody evaluated adds nothing and nothing is normalized or rescaled.
Scores preserve measured criterion points independently of completion or
resource limits. Criteria do not veto the score or approve a run. Completion,
technical validity, artifact evidence, and runtime controls are reported
separately; infrastructure and execution failures still fail the CLI.

The repository is intentionally independent from the `workers` source tree.
Runtime discovery, execution, observation, state access, and cleanup all happen
through functions registered in iii. The only product input is an immutable
subject artifact or an already-running iii stack.

The SWE service suite provides eight isolated engineering
tasks and a continuous eight-ticket journey over the same Python service, with
optional delegation, immutable checkpoints, isolated verification, and a trusted
GitHub handoff.
SWE execution requires Linux with `/usr/bin/bwrap` and enabled unprivileged user
namespaces. CI installs the distribution AppArmor profile needed by Bubblewrap.
Commands and file operations run inside the attempt workspace; controller files
remain outside that boundary.

## Binaries

- `harness-e2e` is started by Compose and registers the asynchronous `e2e::*`
  control plane plus the injectable Console page. Explicit subcommands keep
  direct scenario execution and report inspection available from the same
  binary.

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

List the materialized scenarios and their definition digests:

```bash
cargo run --locked --bin harness-e2e -- list
cargo run --locked --bin harness-e2e -- catalog
```

The four [Registry scenarios](tests/fixtures/registry-version-comparison/README.md)
use the regular scenario catalog, execution flow, and scores: `registry_planning`,
`registry_implementation`, `registry_environment`, and `registry_verification`.
Each has its own [atomic validations](tests/fixtures/registry-version-comparison/scoring.md).

The [Linkly tutorial scenario](tests/fixtures/linkly-tutorial/README.md), `linkly_tutorial`,
runs the seven chapters of the agentic Linkly tutorial plus a project-restart guard as one
scripted dialogue on one Harness session, against the `linkly-agentic` scaffold's own Compose
stack, and scores twenty-two deterministic checks. `scripts/linkly_stack.py` prepares that stack.

The [trending topics build scenario](docs/blog-build-contract.md) uses an isolated
per-attempt Git remote and independent Playwright acceptance against the delivered
SHA. Its [runtime and controls](tests/fixtures/trending-topics-build/README.md)
require Linux amd64, Docker, Git, Python 3, Node and access to the pinned fixture.
Design is free; screenshots are evidence, not an aesthetic score.

Native criteria preserve known awards when dependent checks cannot run. Those
checks have no award and remain `not_evaluated`; an incomplete criterion set has
no total score. Product failures stay technically valid, while infrastructure
failures invalidate the run without erasing prior criterion observations.

Every scenario is a built-in module under `src/scenarios/` that owns its
prompt, setup, deterministic evaluator, and cleanup; the module id is exposed
through the CLI, worker catalog, campaign runner, dashboard, and canonical
result artifacts. Plans select their scenarios explicitly.

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
The engineering handoff uses a protected disposable checkout of its dedicated
pinned revision of `iii-hq/e2e-fixture`. `shell_coder_sandbox`,
`chess_engine_build`, and `trend_blog` prepare their reviewed fixture automatically
from an embedded Git bundle. They require Git, but no fixture checkout or
`HARNESS_E2E_FIXTURE_PATH` configuration. Each attempt operates on a private
workspace; temporary source checkouts are removed after their contents are read.
`typescript_chat_service` carries its own frozen skeleton in the repository and
needs no checkout, but it does require Node 22.6 or newer on the runner host: the
subject's TypeScript application is executed directly through Node type
stripping, both by the public suite and by the runner-owned behavioral probe.
[config/test-plan.json](config/test-plan.json) defines the executable profiles: smoke, regression, capability, evolution,
resilience, endurance, and software-engineering. In the dashboard these profiles are starting templates
for the same plan form and baseline/candidate visualization used by existing plans.
Choose **New plan**, optionally select a template, edit the scope, and select the
execution model.
**Save draft**, **Save and run**, and **Duplicate plan** use one shared lifecycle
and retain native evidence. Fault-injection plans export
to the protected executor. See [executable profile plans](dashboard/README.md#executable-profile-plans).

```bash
cargo run --locked -- test-plan list
```

The `software-engineering` profile selects the seven incremental Kanban cases,
four Registry cases, the trending-topics blog build and the Linkly tutorial,
once each with no technical retries: 13 cases and 13 planned runs. Its twelve
execution groups keep Registry implementation and verification together, in
that order, so verification receives the implementation delivery. Trending
topics runs in its own `case-trending-topics-build` group using the existing
pinned fixture workflow.
Linkly runs its eight exchanges in one `case-linkly-tutorial` group. The executor
creates a fresh pinned `linkly-agentic` scaffold as that group's Compose project,
with baseline worker versions taken from the resolved stack contract.

```bash
cargo run --locked -- test-plan materialize --profile software-engineering
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

Partial GitHub reruns reuse the contract artifact produced by `prepare`, which
is retained for 90 days. The finalizer matches group artifacts to the completed
jobs' execution times, preserving successful groups from earlier attempts.
A rerun that produces no artifact remains missing evidence; an older artifact
does not replace it.

### Agent profiles from Release Control

Release Control can select an existing Directory agent profile for an execution
by including its ID in the frozen plan:

```json
{
  "agent_profile": "console-ui"
}
```

For example, `console-ui` identifies **Console UI Engineer**. The runner reads
the profile from the group's Directory and sends its ID as `agent` to `e2e::run`.
The profile, its parent profiles, skills, functions, and model/provider must
be available in that stack. The runner downloads the stack's versioned skill
bundles into an isolated Directory and waits up to 120 seconds for the selected
profile; profiles that remain unavailable fail resolution.
A profile's model overrides the plan's model; `provider::model` also selects its
provider. Results record the resolved subject model and profile configuration
hash, so changes to an existing profile remain visible between executions.
Each execution keeps the selected profile ID; Run again resolves that ID in its
test stack. Comparisons remain manual in Release Control. Omitting
`agent_profile` keeps the built-in agent.

An execution can also select a project template independently of the test-plan
profile and the agent profile:

```json
{
  "template": "harness",
  "agent_profile": "tech-lead"
}
```

The runner reads the `iii/template.yaml` catalog in `iii-hq/templates` from
`main`, resolves it to one commit before sharding, and records that commit as
`identity.template.revision`. Every group uses that source. The selected
Compose project supplies the base; test-stack versions override its package
selectors, additional packages enter the stack lock, and local workers remain
local. Template skills override whole downloaded namespaces, and its agent
files take precedence over downloaded profiles. Machine-global profiles/skills
are not used when a template or agent override is selected.

The runner also enables the campaign's selected provider when it is absent
from the project, using the provider version pinned in the stack contract.

Scenarios, prompts, permissions, fixtures, seeds and repetitions are unchanged.
The evaluated agent is applied to ordinary sessions and workflow/adaptive
steps; evaluators are unchanged. Linkly retains its pinned task scaffold and
container roles, with the selected template's base and agent assets applied
separately. No template keeps the existing generated stack (or required fixture).

To measure a profile's effect, compare the same plan, template commit, model
and stack with and without `agent_profile`. Changing the template too measures
their combined effect. Non-Compose templates and templates requiring interactive
language choices are rejected before boot. Protected fault groups use an
external supervisor and reject both overrides rather than silently ignoring them.
No new scenario or CLI change is required.

`scripts/report_execution.py` posts what was observed to Release Control's run
ledger over OIDC: `materialized` before anything runs, one `shard` per campaign
group whatever that group did, and a `summary` whatever the finalizer did. Runs
come from `results.json`, or from the journal checkpoints when a group died
before writing one; a group that produced neither still reports, saying so. No
execution is silently lost.

`workers` supplies versioned components of the stack under test; it does not
orchestrate campaigns.

## Console page

Build the worker and its injectable Console page from the repository root:

```bash
cargo build --locked --bin harness-e2e
```

The Rust build creates `dashboard/dist-console/page.js` and `styles.css`, then
embeds both assets in the worker. Node and pnpm must be available on `PATH`.
When Console connects to the same iii namespace, the worker registers those
assets and the `e2e::dashboard::*` read, plan, run, status and cancellation
functions used by the page.

Rust-defined composite scenarios, including the multi-test `security_review`
example, use the current shared result schema and read-only execution projection.

The running Harness must publish request and response schemas compatible with
the current typed surface. Missing or incompatible fields fail preflight; no
payload-version compatibility mode is available.

The page loads data incrementally through iii: 25 compact summaries on the
first overview page, one complete report when an execution is opened, only the
selected pair for comparison, and the model/scenario catalog when the run dialog
opens. Server-side filtering and cursor pagination keep history growth out of
the initial payload. Transport failures stay visible in Console.

The trusted publisher still writes the bounded JSON report archive used by CI
and downstream consumers. It does not publish a Harness E2E web application.
See [dashboard/README.md](dashboard/README.md) for the page contract.

### Compare a local change with Release Control

The Console's Plans page offers **Reference: Release Control** to browse RC history
through the authenticated Release Control browser bridge. Keep the RC tab open,
enable its local Harness connection, and connect it to the same personal Engine
as the Console. The bridge needs the E2E read functions from the companion
Release Control change. No GitHub token or artifact synchronization is needed.

Open a plan to see remote and local executions together with their origin. Select
a reference and a local result to compare their measurements. The scenario links
open the existing A → B comparison with both executions selected. Missing reports and
metrics remain visible as unavailable; reading history creates no local plan.
The comparison runs locally and sends no local results to Release Control.

Choose **run locally** on a remote reference to save its materialized test
parameters as a local plan and run them against your current Harness. Repeating
that action creates a new local plan using the current scenario contracts,
while preserving earlier plans and results. The
reference's scenarios, rounds, repetitions and retry settings come from the
execution's materialization, not from the current profile with the same name.
The current local scenario implementations and Harness are used deliberately:
this is a personal experiment, not an exact-stack certification. No build/Git
tracking or matching remote stack is required. Fault-injection groups still
require the protected executor; they are not silently omitted. References without
shard seeds for every scenario cannot be reproduced. Differences in local
scenario definition or case identity are shown as advisory information.

Results stay in the local plan store. The RC execution remains a reference,
never a locally recreated official execution. Native result validation remains
strict; the remote data is read through the RC API rather than installed as a
native report. Full remote evidence is available through the execution's GitHub
link, subject to its retention; this flow does not download an evidence archive.

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
configuration contains the execution-specific evidence directory and the
separate control-plane database namespace. Start
`worker-compose.control.yaml` before `worker-compose.yaml`: it provisions the
single-connection `harness_e2e` SQLite pool at the configured control-plane
path, with SQL history disabled.
The Harness exits explicitly when that database or its schema is unavailable;
the subject namespace never receives its database client or filesystem path.

Publication validates the locally built binary through a `path://` Compose
container before the package is uploaded. Published campaigns use only exact
Registry package versions. Provider secrets are written to temporary
permission-restricted `env_file` files and are never included in contract,
Compose, evidence, or archive artifacts.

The worker exposes `e2e::run`, `e2e::status`, `e2e::cancel`,
`e2e::results-get`, `e2e::results-list`, `e2e::compare`,
`e2e::scenarios-list`, `e2e::archive`, `e2e::archive-head`,
`e2e::archive-restore`,
`e2e::history-list`, and `e2e::retention-sweep`.
Fault supervisors use `e2e::fault-plan` and `e2e::fault-evaluate` so plan
materialization and recovery classification stay on the same iii control plane.
Subject policies deny `e2e::*`.

### Validating a running worker

`tests/live_worker.rs` talks to a worker that is registered in a real engine,
so its tests are ignored by default. They check that the worker publishes the
catalog this revision materializes (ids, seeds, cases, definition digests),
that it refuses malformed requests without admitting anything, and, when a
subject is named, that one `minimal_path` execution goes through admission,
setup, the subject turn, capture, evaluation and persistence to a technically
valid report readable back through `e2e::results-get`:

```bash
HARNESS_E2E_LIVE_URL=ws://127.0.0.1:49134 HARNESS_E2E_LIVE_NAMESPACE=my-project \
HARNESS_E2E_LIVE_PROVIDER=deepseek HARNESS_E2E_LIVE_MODEL=deepseek-flash \
cargo test --test live_worker -- --ignored
```

Seven scenarios state host paths in their prompts, so their definition digests
only match when the test process carries the worker's `HARNESS_E2E_RUN_DIR`,
`TMPDIR` and `HARNESS_E2E_*_FIXTURE_PATH` values; export the same environment
the Compose file gives the worker. The scenario run leaves one execution
labelled `live worker validation` in the worker's storage.

Durable artifacts are chunked through `storage::*`. Admissions, executions,
runs, attempts and artifact references are written through the control-plane
`database::*` worker. Execution records retain compact dashboard summaries and
observations, so lists and history do not load native reports. Storage carries
no version number and has no migration step: every table records the
fingerprint of the statements that create it, and at start the worker
recreates the tables whose fingerprint moved in one transaction, keeping the
execution records, local plans and receipts it can still read and rebuilding
run projections from the native bundles. Rows it cannot read, missing bundles
and imported Release Control history in a recreated table are logged as
warnings; the history comes back by importing it again, and nothing is
reconstructed as a scored result. A report or plan written under another
results contract is read with a warning, never refused.

Plan definitions and composed execution receipts are stored in `saved_plans` and
`saved_plan_executions` through the database worker. A saved plan or receipt this
binary cannot read is deleted on the next read; plans written by another binary
are never migrated.

Release Control history imports use `harness-e2e-history`, wrapped as
`{json, sha256}` with a `sha256:` digest of the exact UTF-8 JSON. Use **Import
history** in the Console to import a file or explicitly fetch a plan from the RC
bridge. Plans and executions retain source identities, revisions and every
retained report; repeated imports do not create duplicates. Imported active work
never enters local admission or recovery. History remains readable without RC.
Evidence uses local `gh` credentials and Python 3 to verify the GitHub bundle
manifest, execution/attempt identity and file checksums, independently of RC.
Missing, expired, inaccessible and invalid evidence are separate states. Native bundles retain
full reports, manifests and transcripts, loaded on demand for investigation.
The runner has no S3, GCS, R2, SQL-driver, or Harness dependency.

Weekly Stress materializes deterministic fault plans and evaluates journals from a
protected supervisor.
Lane promotion is governed by
[`config/policies/cutover.json`](config/policies/cutover.json).

## Repository boundaries

- `src/` owns the runner, local wire adapters, scenarios, evaluation,
  longitudinal comparison, and the E2E control worker.
- `config/` owns reviewed comparison and cutover policies and fault profiles.
- `tests/` owns test-only fixtures, golden wire schemas, and the Node/Python
  validation suites.
- `schemas/` contains the public contracts for generated E2E artifacts.
- `dashboard/` contains the React, TypeScript, Vite, and Tailwind Console page
  embedded in the worker binary.
- generated reports, transcripts, logs, and deliverables stay outside Git.

The crate may depend on the iii SDK and generic libraries. It must not declare
a path or Git dependency on `workers`, Harness, or another product crate.
Contract compatibility is established at runtime from
`engine::functions::list` and `engine::functions::info`; the checked-in schemas
are parity fixtures, not a linked product API.

The deterministic assessment boundary has one current payload shape, written
only to `results.json`; scenario contracts are the only versioned domain. No
scenario uses a second model: every score and every audit flag is
deterministic.

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
contracts, definition digest, materialized inputs, seed, policies, artifacts,
and raw structural evidence. `e2e::compare` accepts two distinct completed
execution ids (`from_execution_id` and `to_execution_id`) and writes a unique
`comparisons/<comparison-id>/e2e-delta.json` plus `e2e-summary.md`. Numeric
deltas remain disabled when the case set or canonical contract differs.

Deliverable, structural, technical, cost, latency, turns, and retry deltas
remain independent. A case is repeatable after five local runs satisfy the
deliverable, structural, and technical thresholds. Cost and wall-time are
reported as observed metrics and compared only within a compatible
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
