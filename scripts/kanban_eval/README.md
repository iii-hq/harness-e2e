# Kanban local control runner (experimental)

This is an offline control-runner prototype, **not** a registered Harness
`ScenarioId` or a model campaign. It runs a catalog's base or reference revision;
it does not yet execute a subject model or collect model tokens/costs.

`snapshot.py` validates exact commits and parentage, exports the selected tree,
and initializes a fresh one-commit repository without source remotes or future
objects. Archive links are rejected. Catalog metadata and the public prompt are returned to the controller,
not written into the subject workspace.

`run.py` uses an outer Bubblewrap network namespace and a nested candidate
mount/PID namespace. The candidate sees its workspace, runtime and dependencies,
but not the trusted probe, evidence, browser dependencies or host network.
Isolation failure aborts execution; there is no unsandboxed fallback.

Supply administrator-selected local runtime binaries and dependency caches:

```sh
python3 scripts/kanban_eval/run.py \
  --fixture /absolute/path/to/iii-kanban-e2e-fixture \
  --catalog /absolute/path/to/iii-kanban-e2e-fixture/scenarios/catalog.json \
  --case kanban_c2_persistence --revision reference \
  --output /absolute/path/to/new-execution \
  --node /absolute/path/to/node --iii /absolute/path/to/iii \
  --pnpm /absolute/path/to/pnpm-10.18.2 \
  --dependencies /absolute/path/to/kanban/node_modules \
  --browser-dependencies /absolute/path/to/workers/node_modules \
  --browsers /absolute/path/to/ms-playwright
```

The browser cache must match Playwright 1.61.1 (or provide
`--playwright-module` relative to browser dependencies). No dependency download
is attempted. Output must be new. `evidence/` contains provenance, logs and, when
the probe runs, coverage, verdicts and browser screenshots.

`functional_status` describes only executed functional probes. Overall status
remains `incomplete` when required criteria are unverified. A valid negative
control fails functionality; an infrastructure/evaluator failure is **not** a
negative control success. Exit 0 means functional probes passed, not full
acceptance. Do not interpret this prototype as a calibrated benchmark.

Validation on 2026-09-09: snapshot/CLI contract unit tests pass. A real C2 reference
attempt stops at isolation preflight: this host allows one Bubblewrap sandbox,
but denies nested namespace creation. No reference/base functional controls or
browser validations have therefore been established for this runner. No host
security settings were changed.

The broader Python suite subsequently passed 198 tests with `HARNESS_E2E_BIN`
pointing to the existing local Harness executable. The initial errors were due
to the absent default `target/release/harness-e2e` path in this worktree. This
validates the Python suite, not a fresh Rust build or functional Kanban controls.
Additional runner regressions reject interrupted probes, missing coverage,
case mismatches and contradictory verdicts, while accepting valid negative controls.

DeepSeek Flash was confirmed in the local router as `deepseek/deepseek-v4-flash`
on 2026-09-09. No model request was made: the runner still has no subject-execution
integration and isolation must pass before running model-generated code. The
default local Docker sandbox also denied user namespace creation. Do not disable
host or container protections to get a functional result; use a compatible worker.

Before enabling model execution: prove isolation on the target worker, pass all
seven reference controls and fail their bases for the intended feature, complete
criterion coverage, add resource quotas, and wire the runner into Harness's
subject-execution lifecycle. Current timeouts are not disk/memory/process quotas.

```sh
python3 -m unittest discover -s tests/python -p 'test_kanban*.py'
node --check scripts/kanban_eval/probe.mjs
```
