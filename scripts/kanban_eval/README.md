# Incremental Kanban evaluation

Seven native Harness scenarios reproduce the fixture's C1–C7 increments from
their pinned base commits. The `software-engineering` profile combines them with
the four Registry cases, the trending-topics blog build and the Linkly tutorial;
`capability` also includes Kanban, while `smoke` and
`regression` do not. The local controller also runs base/reference
controls without invoking a model, and supports a standalone DeepSeek smoke.
See [VALIDATION.md](VALIDATION.md) for observed results and remaining gates.

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
  --image mcr.microsoft.com/playwright@sha256:cf0daee9b994042e011bc29f20cdff1a9f682a039b43fcd738f7d8a9d3bcd9d6 \
  --fixture /absolute/path/to/iii-kanban-e2e-fixture \
  --catalog /absolute/path/to/iii-kanban-e2e-fixture/scenarios/catalog.json \
  --case kanban_c2_persistence --revision reference \
  --output /absolute/path/to/new-execution \
  --node /absolute/path/to/node --iii /absolute/path/to/iii \
  --pnpm /absolute/path/to/pnpm-10.18.2 \
  --dependencies /absolute/path/to/kanban/node_modules \
  --browser-dependencies /absolute/path/to/workers/node_modules \
  --browsers /ms-playwright
```

The Docker image must already exist locally; its immutable ID is recorded and
used for execution. System libraries and browsers come from the pinned image,
not host `/usr` or `/lib` mounts. `/ms-playwright` is an image-internal path.
Selected Node, iii, pnpm and JavaScript dependencies are mounted read-only.
Docker's default seccomp/AppArmor protections remain enabled. This uses standard
container isolation, not a VM boundary against hostile-kernel exploits.

The image browsers must match Playwright 1.61.1 (or provide
`--playwright-module` relative to browser dependencies). No dependency download
is attempted. Output must be new. `evidence/` contains provenance, logs and, when
the probe runs, coverage, verdicts and browser screenshots.

`functional_status` describes only executed functional probes. Overall status
remains `incomplete` when required criteria are unverified. A valid negative
control fails functionality; an infrastructure/evaluator failure is **not** a
negative control success. Exit 0 means functional probes passed, not full
acceptance. Each criterion depends on explicit functional checks: unrelated
failures do not erase partial credit, and missing/unverified evidence stays
unavailable instead of becoming a candidate zero. A model smoke is not a
comparative benchmark.

Controls and model deliveries use the same source-integrity check. Compose state
and logs live under `/runtime-state/compose`, outside the source workspace. A
new `worker-compose.lock` containing only Compose's canonical empty local-project
lock is recorded as generated output; changes to an existing lock or a lock with
dependencies are still rejected. Git tree IDs compare source contents and modes,
including binary files. Build output and data remain excluded from source capture.

The evaluator records `source-integrity.json` with both tree IDs, changed paths,
generated paths and whether the delivered diff was empty. It preserves
`subject.diff` and `evaluated.diff`. If evaluation is invalidated after functional
checks ran, `functional-result.json` keeps their provisional result without
turning it into an official score. Empty deliveries and broken application
behavior remain functional failures when the evaluator completed successfully.

`instructions.md` supplies the shared native/standalone runtime contract. It
documents iii's injected `_caller_worker_id` metadata and requires changes to be
applied to workspace files. Its digest is part of the native case inputs; the
fixture catalog and historical base/reference commits are not rewritten.

Validation on 2026-09-09: the original Bubblewrap runner stopped at nested namespace
creation. Docker-native separation subsequently passed private-file, parent-PID,
shared-loopback and denied-external-network checks; trusted Chromium also launched
successfully with default container protections. Functional control results must
be recorded separately; these infrastructure checks do not validate the app.

Runner regressions reject interrupted probes, missing coverage, case mismatches
and contradictory verdicts, while accepting valid negative controls.

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
are returned as feedback. A command timeout or output overflow stops candidate
processes and returns bounded partial output with exit 124 or 125, preserving the
workspace for a corrected command. Failed cleanup removes the exact container
and aborts; there is no unbounded or host fallback. No host shell, discovery tools
or child agents are granted.
The standard `agent_trigger` dispatcher is restricted to that single function;
its command contract is supplied explicitly, without namespace discovery.

Unlike control runs, model runs use tmpfs for workspace (256 MiB), data and
runtime state (64 MiB each); no candidate-writable host volume is mounted.
The trusted controller imports only the base snapshot, then captures a diff,
transcript and `subject.json` with observed model/provider, separate usage/cost
fields and duration. Evaluation still runs from the private evaluator container.
Never substitute requested model identity for missing observed identity.
Optional cache counters remain `null` when the provider does not report them.

## Native Harness and Release Control

`HARNESS_E2E_KANBAN_RUNTIME` names the administrator-provisioned runtime JSON.
`bootstrap.py` creates it after validating the fixture checkout/catalog, pulling
the pinned image and installing locked JavaScript dependencies without lifecycle
scripts. It does not install system packages on the host. Docker must be usable
by a non-root runner user.

```sh
HARNESS_E2E_KANBAN_RUNTIME=/absolute/path/to/runtime.json \
  harness-e2e run --provider deepseek --model deepseek-v4-flash \
  --scenario kanban_c2_persistence --output /absolute/path/to/new-report
```

The native setup creates isolated containers and one attempt-scoped command
function. The regular Harness lifecycle collects the model transcript, usage and
cost; the private controller checks the delivered application. The captured
`kanban_evaluation` JSON includes verdict, coverage, runtime provenance, delivered
and evaluated diffs, source-integrity diagnostics, any provisional functional
result, bounded log tails and PNG screenshots encoded as base64. Raw runtime files are private; they
are not subject filesystem artifacts.

To enable this in Release Control's exact-stack workflow:

1. Publish the fixture history through commit
   `0471257a95095da7c5e9d366e26636976472e90d` in the public
   `iii-hq/kanban-e2e-fixture` repository. Checkout uses the workflow's default
   token and never persists credentials.
2. Merge the native scenarios and publish a compatible immutable `harness-e2e`
   worker release. Select that release in the Release Control stack; changing
   `runner_sha` alone does not replace the Registry-resolved worker binary.
3. Materialize the `software-engineering` profile from that runner revision
   (or `capability` for broader coverage). Its seven `case-kanban-*` groups
   provision the fixture/runtime and pass the runtime JSON to the worker before
   calling native `e2e::run`.

Do not declare CI readiness from unit tests alone: complete reference/base
controls, a native real-model attempt, compatible catalog/worker identity and
remote fixture access must all be verified. Evaluator unavailability is not a
candidate zero score; an application that fails build/startup is a task failure.

```sh
python3 -m unittest discover -s tests/python -p 'test_kanban*.py'
node --check scripts/kanban_eval/probe.mjs
```

## Scoring revision 2

The seven native cases now use scenario version 2. `rubric.json` declares stable,
weighted criterion IDs and their evidence checks, totaling 100 points per case.
The frozen inputs include the rubric and its digest; existing execution reports
and fixture revisions are unchanged. Scores from revisions 1 and 2 must not be
compared as though they use an identical acceptance contract.

Each flow records checkpoints before testing a distinct behavior. Passing an
observed checkpoint is retained when a later checkpoint fails. The failed
checkpoint earns zero; later, unexecuted checkpoints are `unverified` and retain
`awarded: null`. The native report retains these per-criterion observations and
leaves the aggregate score unavailable until every criterion is measured; it
does not redistribute missing weights. Application build/startup failures still
fail the deliverable. Controller or source-integrity failures invalidate the
assessment.

Browser probes accept semantic alternatives such as list-based board columns,
server-provided accessible error messages and parent-navigation labels. Negative
controls still reject wrong counts, success-only feedback and inert parent
buttons. Creation, identifier lookup and deletion use independent fixtures.
Pointer-drag checks use an independent ticket and measured initial lane counts.
Draft preservation is tested by preparing a draft before a delayed operation;
a temporarily disabled form is not treated as evidence of lost data.

Regression fixtures reproduce execution `e6f01f0f-1a92-4e5b-b119-b8ca0b84a7a2`:

```sh
python3 -m unittest discover -s tests/python -p 'test_kanban*.py'
cargo test --locked scenarios::kanban::tests
```

These contract tests do not replace Linux Docker reference/base controls or a
new native evaluation of the same delivered snapshot. Re-evaluation should
produce a new linked report, leaving the original evidence and grades intact.
