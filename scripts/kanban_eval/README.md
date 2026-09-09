# Kanban local control runner (experimental)

This is a local control runner with an opt-in, single-model smoke path, **not**
a registered Harness `ScenarioId` or a campaign. By default it runs only the
catalog's base or reference revision and never invokes a model.

`snapshot.py` validates exact commits and parentage, exports the selected tree,
and initializes a fresh one-commit repository without source remotes or future
objects. Archive links are rejected. Catalog metadata and the public prompt are returned to the controller,
not written into the subject workspace.

`run.py` uses two Docker containers with separate mount/PID namespaces. The
candidate has no external network; the evaluator shares only its network namespace
to reach the app over loopback. The candidate sees its dedicated workspace, data,
runtime and dependencies, but not the trusted probe, evidence, browser dependencies
or host network. Both run as the current UID with capabilities dropped, read-only
roots, no-new-privileges and CPU/memory/PID limits. No ports or Docker socket are
exposed. Isolation failure aborts execution; there is no unsandboxed fallback.

Supply administrator-selected local runtime binaries and dependency caches:

```sh
python3 scripts/kanban_eval/run.py \
  --image ubuntu:24.04 \
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

The Docker image must already exist locally; its immutable ID is recorded and
used for execution. Runtime directories and binaries are mounted read-only.
Docker's default seccomp/AppArmor protections remain enabled. This uses standard
container isolation, not a VM boundary against hostile-kernel exploits.

The browser cache must match Playwright 1.61.1 (or provide
`--playwright-module` relative to browser dependencies). No dependency download
is attempted. Output must be new. `evidence/` contains provenance, logs and, when
the probe runs, coverage, verdicts and browser screenshots.

`functional_status` describes only executed functional probes. Overall status
remains `incomplete` when required criteria are unverified. A valid negative
control fails functionality; an infrastructure/evaluator failure is **not** a
negative control success. Exit 0 means functional probes passed, not full
acceptance. Do not interpret this prototype as a calibrated benchmark.

Validation on 2026-09-09: the original Bubblewrap runner stopped at nested namespace
creation. Docker-native separation subsequently passed private-file, parent-PID,
shared-loopback and denied-external-network checks; trusted Chromium also launched
successfully with default container protections. Functional control results must
be recorded separately; these infrastructure checks do not validate the app.

The broader Python suite subsequently passed 198 tests with `HARNESS_E2E_BIN`
pointing to the existing local Harness executable. The initial errors were due
to the absent default `target/release/harness-e2e` path in this worktree. This
validates the Python suite, not a fresh Rust build or functional Kanban controls.
Additional runner regressions reject interrupted probes, missing coverage,
case mismatches and contradictory verdicts, while accepting valid negative controls.

The [control validation report](VALIDATION.md) records all 14 functional controls
and the visual checks. Default Docker protections remain enabled.

## Opt-in DeepSeek Flash smoke

After validating controls, add the following flags to a **base** invocation:

```sh
--subject-model deepseek-v4-flash \
--subject-url ws://127.0.0.1:49134 \
--subject-namespace my-project
```

The endpoint/namespace are local deployment choices. The bridge verifies model
pricing before sending, with a hard US$5 cap, 1,000,000 total tokens, 100 turns,
65,536 output tokens per response and a 1,800-second deadline. The only exposed model tool executes shell commands in
the fixed candidate container, with 120-second/256-KiB limits; nonzero test exits
are returned as feedback. No host shell, discovery tools or child agents are
granted. A failed tool bound removes the candidate container.
The standard `agent_trigger` dispatcher is restricted to that single function;
its command contract is supplied explicitly, without namespace discovery.

Unlike control runs, model runs use tmpfs for workspace (256 MiB), data and
runtime state (64 MiB each); no candidate-writable host volume is mounted.
The trusted controller imports only the base snapshot, then captures a diff,
transcript and `subject.json` with observed model/provider, separate usage/cost
fields and duration. Evaluation still runs from the private evaluator container.
Never substitute requested model identity for missing observed identity.
Optional cache counters remain `null` when the provider does not report them.

This smoke path is not yet the native Harness scenario lifecycle. Complete
criterion coverage and native scenario integration remain necessary before
claiming a calibrated, complete benchmark. Functional success is not full coverage.

```sh
python3 -m unittest discover -s tests/python -p 'test_kanban*.py'
node --check scripts/kanban_eval/probe.mjs
```
