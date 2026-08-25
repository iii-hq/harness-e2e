# Engineering endurance ladder

`engineering_endurance_ladder` v1 measures how far one uninterrupted Harness
session can evolve a real codebase while preserving every accepted behavior.
It is an advisory capability benchmark, not a deployment gate.

## Execution model

- The runner creates a clean, disposable Git repository containing a small
  append-only durable job queue and a green public test suite.
- The session receives only rung 1. It edits production code, tests, commits,
  and calls a run-scoped trusted checkpoint function.
- The checkpoint verifies Git ancestry and scope, runs the public suite and all
  hidden probes through the current rung, and reveals the next ticket only after
  acceptance.
- A rejected checkpoint returns factual evidence to the same session. Three
  rejected attempts on one rung establish the first capability boundary and
  terminate the benchmark normally.
- Accepted commits are immutable ancestors of every later checkpoint. Tests,
  the case manifest, branch, refs, and Git configuration are protected.

The ten cumulative tickets cover idempotency, retry backoff, leases, crash-tail
repair, cancellation, optimistic revisions, legacy migration, atomic
compaction, batch claims, and operational statistics. The execution policy
allows 320 turns, three million total tokens, and a 20-minute stuck timeout;
the scheduled workflow has a four-hour ceiling.

## Longitudinal metrics

The native `engineering_endurance_report` deliverable feeds these observed
measurements into `results.json`:

- `max_accepted_rung`;
- `accepted_tickets`;
- `checkpoint_rejections`;
- `time_to_boundary_ms`;
- `accepted_changed_lines`;
- turns, function calls/errors, checkpoint acceptance ratio and turns per
  accepted rung;
- total tokens and estimated cost when the provider reports them.

The report also retains checkpoint SHAs, attempts, duration, changed paths and
lines, every public/hidden decision, terminal status, and the accepted patch.
A capability failure is a valid measured outcome. Missing terminal evidence or
an invalid accepted Git/test boundary is a hard-gate failure.

## GitHub handoff

The evaluated session has no network or GitHub capability. The trusted
post-run publisher writes a sanitized report and accepted patch to a new
`benchmark-runs/endurance/<execution-id>` branch in `iii-hq/e2e-fixture`, opens
a draft PR, and creates one Check Run per accepted rung plus a neutral boundary
check when the ladder stops. Raw hidden-probe details remain only in the trusted
Harness archive.

The weekly workflow requires the protected `E2E_FIXTURE_GITHUB_TOKEN` secret
with branch, pull-request, and check-run authority on that repository.

## Local validation

```bash
python3 scripts/run_e2e_campaign.py config/campaigns/endurance.json --validate-only
cargo test --lib engineering_endurance_ladder
python3 -m unittest tests.python.test_publish_engineering_endurance
```
