# Real E2E control-plane demo

This demo exercises the extracted system against a running iii stack. It does
not use sample reports, mocked functions, linked Harness crates, or direct
Harness process access.

The default case is `coordination.1`: one child session writes an independent
branch and the root session finalizes one validated deliverable after a real
wake. The run uses a fixed seed so its inputs and expected artifact remain
reproducible. Set `E2E_SCENARIO=coordination.2` to add the next ladder rung with
two parallel children and a merge.

## What it proves

The script performs the complete public flow:

```text
build trusted e2e-worker
  -> register e2e::* in iii
  -> materialize e2e::scenarios-list
  -> admit e2e::run
  -> poll e2e::status
  -> retrieve e2e::results-get
  -> retain results-v2.json, manifest, transcript, evidence, and deliverables
```

The subject sees Harness functions, not the `e2e::*` supervisor surface. The
E2E worker observes and evaluates the run from outside the subject session.
The runner waits on `harness::turn-completed` (internal sink, not `e2e::status`);
the demo script's `e2e::status` poll is only the control-plane execution phase.

## Prerequisites

- a running iii stack at `ws://127.0.0.1:49134` (or `III_URL`);
- the Harness, state, context-manager, and selected model provider registered;
- `cargo`, `iii`, and `jq` available locally.

First verify the real catalog without invoking a model:

```bash
./scripts/demo_e2e_control_plane.sh --catalog-only
```

Run the strict demonstration:

```bash
HARNESS_E2E_WORKERS_REPOSITORY=iii-hq/workers \
HARNESS_E2E_WORKERS_REVISION=<full-subject-git-sha> \
  ./scripts/demo_e2e_control_plane.sh
```

Strict mode requires the versioned Harness wire metadata introduced by the
contract-surface change. During a coordinated migration, an older running stack
can be exercised through the explicit compatibility window:

```bash
HARNESS_E2E_WORKERS_REPOSITORY=iii-hq/workers \
HARNESS_E2E_WORKERS_REVISION=<full-subject-git-sha> \
  ./scripts/demo_e2e_control_plane.sh --allow-legacy-control-plane
```

That flag relaxes only the wire-metadata preflight. Scenario execution,
deliverable validation, lifecycle gates, evidence capture, and cleanup remain
real.

Select another materialized case or model with environment variables:

```bash
E2E_SCENARIO=coordination.2 \
E2E_MODEL=codex/gpt-5.6-terra \
E2E_PROVIDER=openai-codex \
E2E_SEED=4404 \
HARNESS_E2E_WORKERS_REPOSITORY=iii-hq/workers \
HARNESS_E2E_WORKERS_REVISION=<full-subject-git-sha> \
./scripts/demo_e2e_control_plane.sh --allow-legacy-control-plane
```

## Evidence to show

During the demo, point out these checkpoints:

1. `e2e::scenarios-list` returns the version, seed, multidimensional complexity
   profile, required capabilities, artifact schema, and invariants before work
   starts.
2. `e2e::run` immediately returns an execution id; the expensive operation is
   asynchronous and idempotent.
3. `e2e::status` exposes real phase transitions and the active attempt/session.
4. `e2e::results-get` returns the canonical report and immutable artifact
   references only after terminal completion.
5. The run directory contains `results-v2.json`, its compatibility projection,
   the manifest, raw evidence, transcripts, and captured deliverables.

The script exits non-zero when the asynchronous operation completes but the
evaluated scenario does not pass. This distinction prevents a healthy control
worker from masking a subject failure or resource-limit outcome.

Generated files remain outside Git under `target/e2e-demo/`. The worker started
by the script is terminated automatically; the shared iii stack is left
running.

Present the canonical worker output in read-only mode:

```bash
cargo run --locked --bin harness-e2e -- dashboard \
  --view-only \
  --runs-dir target/e2e-demo/runs
```

This view does not expose the dashboard's direct-run HTTP endpoints, so every
execution shown in the demo remains attributable to the `e2e::*` path.

The repository and full SHA are mandatory for a real run. The script refuses to
silently attribute the subject stack to the E2E runner checkout.
