# Local security review E2E

`security_review` is defined entirely in Rust. There is no executable workflow
JSON, editor, draft store, import/export surface, or dashboard endpoint that can
change its flow. Advancement, optional skips and interruption are deterministic.

The scenario contains five semantic tests:

1. `scan_commit_a` validates exact contracts and the fixture, requests the scan
   twice to prove deduplication, polls it, evaluates the report and checks the
   repository stayed unchanged.
2. `suggest_commit_a` runs only when the deterministic scan result contains
   valid findings. Request, polling, patch checks and integrity checks remain
   internal operations of this test.
3. `github_reconciliation` checks cached, refreshed, persisted and filtered
   reconciliation views without combining GitHub and Harness counts.
4. `scheduled_scan_commit_b` creates the manually configured delayed ref, waits
   for cron to originate the run, evaluates it and checks repository integrity.
5. `list_run_history` verifies the completed exact-SHA lifecycle, accounting for
   the optional suggestion test.

Fixture restoration is a mandatory attempt cleanup hook. It is reported
separately and is not modeled as a test.

## Manual local execution

Prepare the stack, fixture clone, Codex provider, GitHub read-only credentials
and cron configuration before starting the runner. Point the fixture variable at
the disposable clone and run:

```bash
HARNESS_E2E_SECURITY_FIXTURE_PATH=/absolute/path/to/fixture \
  cargo run --bin harness-e2e -- security-review \
  --url ws://127.0.0.1:49134 \
  --model codex \
  --provider openai-codex \
  --runs-dir target/security-review-runs
```

The command creates
`<runs-dir>/<execution-id>/results/results.json`. Checkpoints and assets are
persisted below the same results directory per attempt and per semantic test.
The report contains a Rust-generated `flow_snapshot` strictly as evidence; it is
marked `executable: false` and is never accepted by the CLI or API.

## Read-only tracking

Start the local dashboard on port 4173 against that runs directory:

```bash
cargo run --bin harness-e2e -- dashboard \
  --listen 0.0.0.0:4173 \
  --runs-dir target/security-review-runs \
  --view-only
```

Open `#/security-review/<execution-id>`. The GET-only projection displays each
semantic test with status, duration, metrics, assets, hard gates, evaluations,
failures and skip reason. The canvas is non-editable and an equivalent table is
available for keyboard and assistive-technology navigation.

This local workflow does not modify CI, GitHub Actions, release gates or
publication configuration.
