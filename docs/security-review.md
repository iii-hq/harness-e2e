# Composite scenarios and the local security review E2E

`security_review` is a normal scenario in the shared catalog and the first
consumer of the generic composite-scenario driver. Future scenarios can register
their own Rust definition, step descriptors, executors and mandatory cleanup in
the same driver. There is no executable workflow JSON, editor, draft store,
import/export surface, special route, or dashboard endpoint that can change a
flow. Advancement, optional skips and interruption are deterministic.

The scenario contains five semantic tests:

1. `scan_commit_a` validates exact contracts and the fixture, requests the scan
   twice to prove deduplication, polls it, evaluates the report and checks the
   repository stayed unchanged.
2. `suggest_commit_a` runs only when the deterministic scan result contains
   valid findings. Request, polling, patch checks and integrity checks remain
   internal operations of this test.
3. `scheduled_scan_commit_b` creates the manually configured delayed ref, waits
   for cron to originate the run, evaluates it and checks repository integrity.
4. `github_reconciliation` checks cached, refreshed, persisted and filtered
   reconciliation views without combining GitHub and Harness counts.
5. `list_run_history` verifies the completed exact-SHA lifecycle, accounting for
   the optional suggestion test.

Fixture restoration is a mandatory attempt cleanup hook. It is reported
separately and is not modeled as a test.

## Manual local execution

Prepare the stack, fixture clone, a provider such as Codex, GitHub read-only
credentials and cron configuration before starting the runner. The provider is
ordinary CLI subject configuration; it is not embedded in `security_review`.
Point the fixture variable at the disposable clone and use the same `run`
command as every other scenario:

```bash
HARNESS_E2E_SECURITY_FIXTURE_PATH=/absolute/path/to/fixture \
  cargo run --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-terra \
  --provider openai-codex \
  --scenario security_review \
  --runs-dir target/security-review-runs
```

The common CLI prints the resulting report path and creates
`<runs-dir>/<execution-id>/results/results.json`. Checkpoints and assets are
persisted below the same results directory per attempt and per semantic test.
The report contains a Rust-generated scenario-flow snapshot strictly as
evidence; it is marked `executable: false` and is never accepted by the CLI or
API.

## Read-only tracking

Start the local dashboard on port 4173 against that runs directory:

```bash
cargo run --bin harness-e2e -- dashboard \
  --listen 0.0.0.0:4173 \
  --runs-dir target/security-review-runs \
  --view-only
```

Open the execution from `#/overview`, like any other retained run. Its normal
`#/execution/<execution-id>` detail displays every semantic test with status,
duration, dependencies, metrics, assets, hard gates, evaluations, failures and
skip reason. The projection is read-only and includes an equivalent table for
keyboard and assistive-technology navigation. Any future composite scenario
with `semantic_tests` receives the same presentation automatically.

This local workflow does not modify CI, GitHub Actions, release gates or
publication configuration.
