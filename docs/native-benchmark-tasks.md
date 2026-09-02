# Native benchmark tasks

Native tasks are the stable corpus used to measure how Harness capability,
efficiency, convergence and reliability change between system identities. A
task owns its instruction, immutable fixture, execution envelope and
deterministic verifier; it is not a projection of a Rust scenario.

The pilot corpus contains two multi-file bug fixes, one feature, one code
review, one implementation plan and one operational recovery. Contracts live
under `tasks/*/task.toml`; runner-private verifier specifications are adjacent
JSON files, and instructions are the only task-authored text sent to Harness.

## Execution

```text
compile task
  -> verify immutable source
  -> materialize a disposable Git workspace
  -> prove the expected baseline state
  -> execute one Harness turn in that filesystem scope
  -> run deterministic public and private verification
  -> persist task-result.json
  -> teardown the complete Harness session tree
```

Run the offline compiler and fixture validation:

```bash
cargo run --locked -- tasks list
cargo run --locked -- tasks validate
```

Execute one task against a Compose-managed Harness stack:

```bash
cargo run --locked -- tasks run bugfix_config_precedence \
  --url ws://127.0.0.1:49134 \
  --model codex/gpt-5.6-luna \
  --provider openai-codex
```

Execute the complete pilot suite:

```bash
cargo run --locked -- tasks run-suite \
  --suite config/task-suites/pilot.json \
  --model codex/gpt-5.6-luna \
  --provider openai-codex
```

Compare two compatible results:

```bash
cargo run --locked -- tasks compare \
  --baseline baseline/task-result.json \
  --candidate candidate/task-result.json
```

## Verifier profiles

- `code_patch` requires a red baseline, exact mutation scope, patch bounds,
  public tests and runner-private probes.
- `structured_artifact` allows only one declared artifact and validates its
  JSON schema plus evidence-bound assertions. It powers review and planning.
- `state_recovery` validates a recovered state and report against the immutable
  initial snapshot, including forbidden-operation evidence.

## Longitudinal identity

`case_fingerprint` binds task behavior, fixture manifest, verifier, model,
provider and execution limits. `system_identity_sha256` separately binds the
observed Engine and Harness versions. Comparisons are causal only when case
fingerprints match and both executions have valid infrastructure evidence and
complete verifier coverage. Incompatible executions retain their outcomes but
do not emit capability, cost or latency deltas.

Task outcome keeps `product_passed`, `infrastructure_valid`, `budget_passed`
and `coverage_complete` separate. Tokens, turns, function-call errors and wall
time are retained as independent longitudinal metrics rather than collapsed
into one score.

The current pilot fixtures are embedded snapshots so development does not
depend on unpublished Git state. Before merging a future external-fixture
cutover, publish the selected subtrees in `iii-hq/e2e-fixture`, pin one full
commit SHA and update the source manifests; do not create another fixture
repository.
