# Versioned multi-step workflows

The local runner accepts a versioned, acyclic workflow definition whose step
implementations are registered Rust types. JSON selects a registered
`step_type@step_version`, supplies schema-validated configuration, and binds
typed output ports from ancestor nodes. It cannot name a function dynamically
or provide executable code.

## Commands

```bash
cargo run --bin harness-e2e -- workflow list
cargo run --bin harness-e2e -- workflow validate \
  --file config/workflows/security-scan.full.json
cargo run --bin harness-e2e -- workflow run \
  --file config/workflows/security-scan.full.json \
  --url ws://127.0.0.1:49134 \
  --model MODEL_ID \
  --provider PROVIDER_ID \
  --output target/security-scan-e2e
```

Validation materializes the complete graph before execution. It rejects
cycles, missing or non-ancestor bindings, incompatible port types, unknown
step versions, invalid node configuration, impossible limits, conditional
required nodes, unsafe retry policies, and criteria whose weights do not total
100. Ready nodes execute concurrently with a deterministic id tie-breaker.
`dependency_policy: terminal` provides a join that can collect a skipped
optional branch; the default `succeeded` policy requires successful ancestors.

Every Harness node receives a new session. Inputs are bounded, redacted and
attached as untrusted structured JSON. One `harness::turn-completed` binding is
created per workflow attempt, while independent broadcast subscriptions keep
events and timeouts isolated per session. A technical failure stops scheduling,
cancels active sessions and runs cleanup. `technical_retries` restarts the
whole workflow and is valid only when every node is idempotent.

Each node completion, skip or failure atomically rewrites
`checkpoints/<run>/<attempt>/workflow-checkpoint.json`. Assets are redacted,
hashed and persisted before node teardown under
`deliverables/<run>/<attempt>/<step>/<asset>`. JSON and UTF-8 text are
supported; binary assets and automatic crash resume are deliberately outside
v1. A later invocation begins at the start and the interrupted checkpoint
remains evidence.

`results.json` is written as schema version 2. The reader accepts historical
unversioned results as v1 and normalizes them to a synthetic one-step workflow,
but the writer emits only v2. Canonical step evidence lives in
`workflow_runs[].steps`; aggregate transcript/session fields remain populated
only for a primary Harness session.

## Local editor

Run `harness-e2e dashboard` and open `#/workflows`. Draft definitions live in
`<runs-dir>/workflow-drafts` and are written atomically. Canvas layout is stored
beside, not inside, the executable definition and therefore does not affect its
hash. The editor uses the Rust catalog and JSON Schemas, blocks obvious cycles
and incompatible connections in the browser, and always validates again on the
server. Execution copies the exact validated definition into the run directory;
subsequent draft edits cannot change an active run. The editor is intentionally
absent in `--view-only` mode.

The canvas has an equivalent table view and keyboard operations for selection,
deletion, undo and redo. Official definitions are read from
`config/workflows`; export downloads canonical JSON and never writes the Git
worktree.

## Reusable sequential patterns

Loops are statically unrolled and bounded. Useful shapes include:

- produce two candidate assets in parallel, evaluate both, select
  deterministically, refine once, then reevaluate;
- plan a migration, render SQL and a rollback asset, validate both in parallel,
  run a disposable apply, then join operational and data-integrity evidence;
- fan out research collection, synthesize only sanitized references, run a
  deterministic citation audit, repair once and publish the final assessment;
- materialize a release candidate, inspect provenance and policy in parallel,
  create an optional diagnostic branch on failure, then join without allowing
  an AI assessment to promote a failed gate.

New integrations or evaluators require a new immutable Rust descriptor and
executor. This keeps paths, policies, function IDs and success decisions out of
editable JSON.

## `security-scan.full`

The checked-in workflow covers scan request and deduplication, completion and
report validation, optional suggest, disposable `git apply --check`, cached and
live GitHub reconciliation, filters/pagination, fixture integrity, delayed ref
creation, observation of a cron-created run, final listing and bounded cleanup.

The external launcher must provide an isolated clone through
`HARNESS_E2E_SECURITY_FIXTURE_PATH`. The configured repository id is fixed as
`iii-hq/security-scan-e2e-fixture`, and the clone must initially be clean with no
`security-scan-e2e-scheduled` ref. A template is available in
`tests/fixtures/security-scan-repository`; all credential-shaped values are
explicitly fake and no workflow in that template performs work.

The local stack must pin exact revisions of `security-scan`, `state`, `queue`,
`worktree`, `configuration`, `cron`, `github`, Harness and the provider. Use a
`state` revision that exposes the private `state::claim-namespace` contract;
do not replace it with public state functions. Boot `security-scan` once with
its empty shipped config so it registers the schema, then set the full
`security-scan` value with `configuration::set` and restart only that worker,
because this worker does not hot reload. Configure its cron expression for
10-15 seconds and target `refs/heads/security-scan-e2e-scheduled`; the workflow
creates that ref only when the schedule assertion begins.

The GitHub worker needs a read-only token limited to Dependabot and code-scanning
read endpoints. Authentication, permissions or external availability failures
are infrastructure evidence, not scanner-quality regressions. Harness findings
remain exact-commit evidence, while Dependabot and code-scanning counts retain
their independent repository scopes and are never summed.

This repository does not create the private GitHub fixture or seed its alerts.
Those are operator-owned external resources. It also does not change CI or
release gates; the workflow is a local execution target.
