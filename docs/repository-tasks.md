# Declarative repository tasks

Repository tasks describe code-oriented benchmark fixtures separately from the
restricted Markdown scenario format. They are compiled from
`repository-tasks/*/task.toml`, hash the adjacent `instruction.md`, and remain
non-authoritative during the initial shadow rollout.

The first two contracts shadow `shell_coder_sandbox` and
`performance_regression`. The subject executes each existing built-in scenario
once. Its legacy evaluator remains authoritative, while the repository-task
verifier projects the same normalized observations through the declarative
assessment contract. The runner stores the comparison as advisory evidence;
absence, failure, or disagreement cannot change score, status, hard gates, or
cleanup classification.

## Fixture sources

`git_checkout` requires a full Git revision, repository id, safe subtree,
content manifest, environment variable containing an absolute checkout, and an
exact file inventory. The runner verifies all of them before emitting shadow
evidence. `shell_coder_sandbox` uses the existing `iii-hq/e2e-fixture` checkout
fetched by the campaign workflow at the contract's exact revision. The
authenticated source checkout is never exposed to the subject: the protected
launcher removes its remote and retains only the execution branch in the
disposable workspace.

`embedded_directory` addresses a safe repository-relative directory and binds
its exact file inventory and content manifest. `performance_regression` uses
the existing `tests/fixtures/performance-regression` directory, so it does not
need another Git repository.

Neither source permits symlinks, parent traversal, absolute task paths, unknown
manifest fields, incomplete revisions, mutable dependency resolution, or an
unbounded execution policy.

## Validation

Run the compiler without a model or stack:

```bash
cargo run --locked -- validate-repository-tasks
```

The checked-in JSON schema is `schemas/repository-task-v1.json`. Behavioral
identity includes the complete parsed definition and exact instruction bytes.
Changing either requires a task version review before the task can leave
shadow mode.

## Shadow evidence

Each completed legacy evaluation may add a
`harness-e2e-repository-task-shadow/v1` JSON artifact under the attempt's
`evidence/` directory. It records task and verifier identity, observed fixture
identity, legacy and generic projections, and explicit mismatches. These
artifacts are suitable for parity calibration but are excluded from verdicts
and longitudinal scoring.
