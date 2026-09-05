# SWE service scenarios

The SWE suite exercises everyday engineering on one Python standard-library
profile service: HTTP and CLI interfaces, SQLite, configuration, cache, event
replay, and a legacy client. The separate `iii-hq/e2e-fixture` repository owns
the source snapshots. The executor embeds its dedicated immutable Git bundle;
the older shared and engineering-ticket fixture pins are unchanged.

## Tickets and entry states

| Scenario | Entry snapshot | Task |
| --- | ---: | --- |
| `swe_config_isolation` | 00 | Configuration precedence and independent CLI overrides |
| `swe_cache_invalidation` | 01 | Version-aware invalidation and live cache limits |
| `swe_batch_replay` | 02 | Ordered bounded replay, initial cursor, empty and invalid input |
| `swe_replay_recovery` | 03 | Idempotent recovery after an actual post-commit process interruption |
| `swe_contract_migration` | 04 | API migration with a legacy client revealed after the first submission |
| `swe_tenant_isolation` | 05 | Identity-bound reads and mutations across tenants |
| `swe_replay_performance` | 06 | Bounded SQL work without changing results or paging |
| `swe_release_handoff` | 07 | Override removal, inherited settings, and delivery documentation |
| `swe_service_journey` | 00 | All eight tickets in one continuing session and repository |

Each isolated case starts from the reference solution of the preceding tickets.
The journey keeps the subject's own code after each accepted checkpoint. It
never replaces a failed implementation with a reference solution or skips a
failed ticket. Only the selected snapshot is exported into a fresh repository;
the source bundle, future snapshots, and trusted verifier are outside the subject
workspace.

## Subject workflow

The subject receives only its current request and the existing public contracts.
It can investigate, edit production code and documentation, add standard-library
unittests under `tests/agent`, and delegate when useful. The main session owns
integration. The benchmark protects `tests/reference`, Git configuration/refs,
and its control files. Checkpoints evaluate immutable committed exports, run
reference and authored tests, and preserve the previously accepted Git prefix.

The run-scoped checkpoint function accepts `ticket`, `head` (the complete Git
SHA), and optional `revision_id`. Responses distinguish acceptance, rejection,
new requirements, completion, and a capability boundary. Repeating a submission
is idempotent. Three distinct rejected submissions on a ticket end that case.

On the first valid ticket-5 submission, the controller reveals and runs the
legacy canary. This is not a rejected attempt. The subject acknowledges the
returned `revision_id`; an already-compatible implementation can submit the
same commit. The next ticket is released only after the current one is accepted.

## Execution environment

Use a dedicated stack whose shell/coder worker sees only the subject workspace
parent, mounted at the same absolute path as on the trusted runner. Set
`HARNESS_E2E_SWE_WORKSPACE_ROOT` to that parent. The runner's output, state,
source bundle and verifier must be outside the mount. The subject receives no
provider or GitHub credentials. This is a runtime requirement: a host shell's
`fs_scope` or confined working directory does not isolate arbitrary commands.

Before a model is called, the executor verifies that the shell can read its
workspace but cannot read a random trusted canary. It also verifies the entry
snapshot through the OS-isolated grader. An unsuitable environment fails as
infrastructure; there is no unconfined fallback.

Linux supports bubblewrap with user/mount/PID/network namespaces. The fallback
uses a cached official Python Docker image with read-only candidate and verifier
mounts, private temporary storage, and no external networking. macOS requires
that Docker backend. The wrapper never pulls an image during an evaluation.
CI provisions an immutable official Python image before mandatory isolation
tests.

The grader is a conventional Python behavioral test runner inside that OS
boundary. It protects the controller and future source snapshots; it does not
claim resistance to deliberately malicious Python introspecting assertions in
the same interpreter. These scenarios evaluate software engineering behavior,
not arbitrary hostile-code containment.

The journey has a 90-minute execution deadline, 320 generations, and 1.5 million
input/output tokens across the root and descendants. Isolated cases allow
15 minutes, 64 generations, and 250,000 tokens. Capture and cleanup have an
additional shared five-minute limit. Cancellation, a resource limit, or a
capability boundary retain both the accepted prefix and unfinished diff.

## Running and inspecting

Build with the repository's supported Rust toolchain, Node and pnpm:

```sh
cargo +1.97.1 build --locked --release
target/release/harness-e2e list
target/release/harness-e2e catalog
```

With the isolated stack and a configured model already available:

```sh
export III_URL=ws://127.0.0.1:49144
export III_NAMESPACE=swe-pilot
export HARNESS_E2E_SWE_WORKSPACE_ROOT=/absolute/subject-workspaces
export HARNESS_E2E_MODEL=codex/gpt-5.6-luna
export HARNESS_E2E_PROVIDER=openai-codex

target/release/harness-e2e run \
  --model "$HARNESS_E2E_MODEL" --provider "$HARNESS_E2E_PROVIDER" \
  --scenario swe_config_isolation --runs 1 --technical-retries 0 \
  --output target/swe-one

python3 scripts/run_e2e_campaign.py config/campaigns/swe-continuous.json \
  --e2e-bin "$PWD/target/release/harness-e2e" \
  --execution-id local-swe-journey --output-root target/swe \
  --summary target/swe/summary.json
```

`swe-isolated` contains eight separate groups; `swe-continuous` contains the
journey. Both use one run, zero whole-case retries, and advisory campaign policy.
Release Control owns operational scheduling and dispatch through the existing
exact-stack entrypoint. The deployment must provide the workspace-isolated
shell and the grader backend described above; normal unconfined stacks are
intentionally unsuitable.

Each case writes `deliverables/<attempt>/swe_service_report.json`, including the
fixture revision, initial and accepted commits, checkpoint history, terminal
state, accepted patch, and unfinished patch. The same deliverable is referenced
from `results.json`, including on deadline/cancellation after preparation.
The exact-stack launcher preserves native report bytes before removing the
disposable stack.

The independent publication step calls `scripts/publish_swe_campaign.py` with
only the preserved SWE report directory. It creates a draft PR in `e2e-fixture`
for each journey, containing a whitelist-sanitized report and accepted patch.
It omits raw verifier output and the unfinished patch. Repeating publication
reuses matching evidence and the PR; a partial branch-without-PR write can
recover. Publication failures get their own receipts and never change the
scenario result. Isolated cases and empty report directories do not publish.

## Validation

```sh
cargo +1.97.1 test --locked --all-targets
cargo +1.97.1 clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s tests/python -p 'test_swe_service_*.py'
python3 -m unittest discover -s tests/python -p 'test_publish_swe*.py'
python3 scripts/run_e2e_campaign.py config/campaigns/swe-isolated.json --validate-only
python3 scripts/run_e2e_campaign.py config/campaigns/swe-continuous.json --validate-only
```

Fixture qualification defaults to the packaged bundle, with no sibling checkout
dependency. Fixture authors can set `SWE_FIXTURE_ROOT` to test a new checkout
before committing and re-pinning it. Set `SWE_REQUIRE_OS_ISOLATION=1` for mandatory
OS isolation qualification; unsupported hosts fail instead of silently skipping
that required evidence.
