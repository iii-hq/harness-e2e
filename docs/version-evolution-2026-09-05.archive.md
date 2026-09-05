> Registro histórico preservado em 2026-09-05. O conteúdo abaixo é o rascunho
> anterior, com observações locais não revalidadas. Não define o plano vigente.
> Consulte o [plano mestre](e2e-test-plans.md), inclusive suas correções
> de cobertura, contratos e interpretação estatística.

# Version evolution plan

The `daily`, `weekly` and `post-deploy` plans answer "did this version break
anything?". This plan answers a different question: **which way is Harness
capability moving from one version to the next?**

It runs once per version, in two layers with distinct roles, and produces a
comparable longitudinal series.

## Target: the branch, not main

The plan executes from the worktree
`/home/layon/workspaces/harness-e2e-benchmarks`, on branch
`feat/cohort-metric-comparison`.

That is not a preference. `git log feat/cohort-metric-comparison..main` comes
back empty: the branch is a **strict superset** of main. `src/longitudinal.rs`,
`src/control.rs`, `scenarios/` and `config/campaigns/` are identical between the
two — every piece of cohort machinery main offers is already there — and the
branch adds the native task subsystem. Running main would give up one layer and
gain nothing.

## The two layers

| Layer | Measures | How | Needs the `harness-e2e` worker |
| --- | --- | --- | --- |
| **Tasks** | code, feature, review, planning, recovery | deterministic verifier, enforced mutation scope | no |
| **Scenarios** | durable state, timers, concurrency, adaptive | `e2e::*` control plane, LLM judge on Markdown | yes |

The split came from measurement, not taste. The task layer has deterministic
verification and an enforced patch scope, so it measures code better than the
equivalent scenario. The scenario layer covers what **no task reaches** — and
that is exactly where the strongest calibration finding appeared.

### Layer 1 — native tasks

Seven tasks, five repetitions, suite `config/task-suites/development-repeatable.json`.

```bash
III_NAMESPACE=my-project \
HARNESS_E2E_TASK_FIXTURE_PATH=/home/layon/workspaces/e2e-fixture \
./target/debug/harness-e2e tasks run-suite \
  --suite config/task-suites/development-repeatable.json \
  --model codex/gpt-5.6-terra --provider openai-codex \
  --url ws://127.0.0.1:49134
```

`III_NAMESPACE` is not optional. Without it the CLI resolves in the `default`
namespace and all seven tasks fail instantly on
`harness::send: Function harness::send not found in namespace default`.

The fixture is a single variable, `HARNESS_E2E_TASK_FIXTURE_PATH`, pinned to
revision `5e9bf8fd` — where the local `e2e-fixture` checkout already sits.
Against the five independent fixture causes the scenario layer needs, that is
the difference between running and not running.

Each task carries a versioned contract (`task.toml`), an instruction, a
development verifier and, in an official run, a runner-private verifier overlay
whose bytes never enter the subject filesystem. The contract enforces scope:
`allowed_paths`, `protected_paths`, `minimum/maximum_changed_files` and
`maximum_patch_lines`.

Each run's result carries `product_passed`, `structural_integrity`,
`grounding_integrity`, `technical_failure`, `budget_passed`, `coverage_complete`
and `cleanup_valid`, plus `harness_version`, `engine_version`,
`system_identity_sha256` and `cohort_identity_sha256` — the version axis of the
series.

### Layer 2 — scenarios

Six blocks, three runs per case, through the `e2e::*` control plane. A block is
one durable execution and also the retry boundary: non-replay-safe scenarios
never share an invocation with replay-safe ones.

| Block | Capability | Cases | Retries |
| --- | --- | --- | --- |
| `evo-core` | minimum path and overhead | 8 | 1 |
| `evo-durable` | durable state, timers, validation, cleanup | 8 | 1 |
| `evo-concurrency` | fan-out, fan-in, contention, coordination | 8 | 1 |
| `evo-code` | code that no task covers | 5 | 1 |
| `evo-integration` | versioned services and adversarial robustness | 3 | 1 |
| `evo-adaptive` | replanning against invalidating evidence | 2 | 0 |

Total: 34 cases, 102 runs. The exact composition lives in
[`config/plans/version-evolution.json`](../config/plans/version-evolution.json).

Six scenarios left the plan because the task layer measures them better, and the
manifest records the reason for each one under `ceded_to_task_layer`:
`shell_coder_sandbox`, `engineering_ticket`, `trend_blog`,
`cross_repo_contract_migration`, `release_train_recovery` and `security_review`.
The last three were, on top of that, the ones failing on environment.

## What stays pinned

`e2e::compare` only accepts two executions whose cohort identity matches. Any
divergence drops the comparison to `comparable: false`, so these fields belong to
the plan rather than the operator:

| Field | Value |
| --- | --- |
| `lane` | `deployed-evolution` |
| subject | `openai-codex` / `codex/gpt-5.6-terra` |
| judge | `openai-codex` / `codex/gpt-5.6-sol` |
| `stack_mode` | `source` |
| `seed` | the scenario's canonical seed |
| `runs` | 3 (scenarios) / 5 repetitions (tasks) |

The lane name is not cosmetic: `lane_budget` resolves the budget by substring. A
lane with no recognized keyword falls into the default bucket and the control
plane rejects `runs=3` with *"permits 1 to 1 runs per case"*.

The only thing that **must** vary between two runs of the series is the system
under test: `harness_version`, `engine_version` and the contract hashes.

## Sampling and maturity

Both layers use the same thresholds: rate metrics at any n, `flaky_rate` from 2
runs, tail metrics (`p95_*`) only from **20**. Maturity is `directional` below 5,
`repeatable` from 5, `validated` from 20.

So the task layer, at 5 repetitions, delivers `repeatable`. The scenario layer,
at 3, delivers `directional`. Neither produces `p95`, and the relative cost, time
and turn gates stay advisory.

This series is an instrument of **direction**, not a promotion gate.

## Cost: tokens, not dollars

`session_cost_usd` comes back `0.0` on every run with `openai-codex`, in both
layers. The dollar cost dimension is dead with this provider.

The task layer works around it by measuring `p50/p95_total_tokens` and
`p50/p95_billable_tokens`, plus `function_calls` — an efficiency signal the
provider actually reports. The scenario layer has no equivalent: there, wall time
and turns are what remain.

## The finding calibration produced

Five independent scenarios, across two different blocks, failed with the same
signature: `made no observable progress for Ns while waiting for the complete
session tree`. They are `timer_wake` and `wake_chain_soak` (`evo-durable`), and
`quorum_fan_in`, `fanout_ladder` and `depth_ladder` (`evo-concurrency`) — all of
them dependent on wakes or child sessions, with different timeouts (120s, 360s,
420s) and all stopping at the same point.

Five distinct scenarios stopping on the same signal is not statistical
dispersion; it is one cause. For contrast, the task that ran clean closed with
`tree_complete=true` in 11 turns.

That finding exists only because the scenario layer exists. None of the seven
tasks touches a wake, a timer or a fan-out.

## Calibration numbers

From the 2026-09-04 scenario run against `harness-1.8.8-rc.3`:

- wall time per run: median **80s**, maximum 805s;
- `minimal_path` costs ~168s median for 2–3 turns — the cheapest overhead
  indicator in the series;
- 12 cases failed a hard gate, 10 passed clean, 5 stalled, 2 raised
  `subject_error`, 11 were blocked by environment.

From the task cohort, 35 runs, same day, same version: **35 of 35 completed,
zero lost to the environment**, 25 passed the product, every task at
`repeatable` maturity. Two tasks failed 0/5, deterministically —
`security_code_review` and `release_train_recovery_simulated`.

## Open blockers

- **The compose file carried a broken container.** An incomplete
  `harness-e2e-benchmarks:` entry with no start command made every compose
  operation fail with `MISSING_START_COMMAND`. Removed.
- **The readiness probe overran its budget by 200ms.** The worker waits for
  `state::list` on scope `harness_e2e_execution` with a 1000ms timeout; the call
  takes ~1210ms once durable history grows, so the worker never registered. The
  probe now allows 15s per attempt with a 60s deadline — an **uncommitted**
  change in `harness-e2e-benchmarks/src/worker.rs`.
- **`security_review` as a scenario needs a `security-scan` worker** that is
  declared nowhere. The `security_code_review` task covers the same axis without
  that dependency — one more reason for the cession.
- **Markdown cleanup leaked five tables** in the database during
  `database_migration_recovery`. None were dropped.

## Out of the plan, and why

- `engineering_endurance_ladder` — 320 declared turns and hours per run; it stays
  on the endurance track with its own cadence.
- `browser_cross_site` — real browser and multi-origin fixture; environment
  fragility contaminates the series.
- `todo_worker_simple` and `todo_worker_planned` — they mutate the worker
  registry.
- `chess_play_ladder` — 128 turns with high dispersion.

The `daily`, `weekly` and `post-deploy` plans remain the owners of fast
regression detection, in [`docs/e2e-test-plans.md`](e2e-test-plans.md). This plan
does not replace them.
