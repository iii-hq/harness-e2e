# Harness improvement loop

The local improvement supervisor optimizes Harness while keeping the E2E
runner, cases, seed, model identities, binary stack and acceptance policy
frozen. Advisor and judge output is consultative; deterministic gates and the
longitudinal comparison own the decision.

## Prepare a spec

Copy `config/improvement-loop.example.json` and replace every placeholder. Use
full Git revisions and absolute paths. The controller engine and Harness
versions must match `engine::health::check` and `engine::workers::list`.
The `harness-e2e` checkout used to build the supervisor must be clean so that
`e2e_revision` identifies the exact runner bytes; the Workers checkout may be
dirty because baseline and candidates are always created from `base_revision`
in isolated worktrees.

The `stack.binary_sha256` map is mandatory. It binds the engine plus every
non-Harness binary shared by incumbent and candidate. Compute each value from
the exact file named by the spec, using `sha256sum`, and prefix the digest with
`sha256:`.

The v1 pilot rejects drift from seed 4404, five samples, one outer technical
retry, the fixed target and four sentinels. The runner's existing safety policy
still applies: replay-safe cases receive the retry, while the non-replayable
`policy_bound_action` case receives zero. The supervisor merges both internal
groups into one canonical `results.json`; baseline and candidate therefore
remain directly comparable.

## Run from the CLI

```text
harness-e2e improve start --spec /absolute/path/to/improvement-loop.json
harness-e2e improve status --runs-dir /absolute/path/to/runs --loop-id <id>
harness-e2e improve resume --runs-dir /absolute/path/to/runs --loop-id <id>
harness-e2e improve cancel --runs-dir /absolute/path/to/runs --loop-id <id>
harness-e2e improve report --runs-dir /absolute/path/to/runs --loop-id <id>
```

`start` and `resume` run to a terminal state. `status` may omit `--loop-id` to
list all loops. `report` expands the Advisor input and answer, proposal, diff,
checks, comparison and decision from verified artifact references.

## Run from the dashboard

Mutable routes do not exist unless explicitly enabled:

```text
harness-e2e dashboard \
  --runs-dir /absolute/path/to/runs \
  --enable-improvement-loop
```

The Plans area can create a loop from a complete JSON spec, start, cancel and
resume it, and inspect the direct Advisor answer, evidence identities, target
and sentinel metrics, candidate diff, checks and transition timeline. The
Console/Registry worker registration never enables these host mutations.

## Persistence and recovery

Each loop lives under `runs_dir/improvement-loops/<id>`. `loop.json`,
`spec.json` and `journal.json` are replaced atomically. Advisor envelopes,
redacted traces, patcher traces, diffs, check logs, comparisons and decisions
are immutable content-addressed references. Native E2E outputs remain
`results.json`, `manifest.json` and `observation.json`; the supervisor does not
introduce a second result format.

Explicit resume verifies artifacts, LocalPlan lock, worktree branch and HEAD.
Missing or divergent state becomes `needs_reconciliation`. Candidate branches
and rejected worktrees are retained locally; the supervisor never pushes,
opens a PR, publishes or promotes.
