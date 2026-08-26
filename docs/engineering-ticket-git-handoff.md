# Engineering ticket Git handoff fixture

`engineering_ticket_git_handoff` v3 runs against a disposable, offline Git
clone. The campaign workflow never creates a checkpoint commit: the planner and
implementer Harness sessions own all commits made after the reviewed baseline.

## Verdict and calibrated score

The handoff scenario keeps correctness and protocol integrity as deterministic
hard gates. Passing still requires orchestration discipline, linear Git
checkpoints, ticket acceptance, and bounded scope/lifecycle behavior.

The 100-point score is intentionally more discriminating:

- orchestration discipline: 15 hard-gated points;
- Git handoff integrity: 20 hard-gated points;
- ticket acceptance: 35 hard-gated points;
- scope and lifecycle: 10 hard-gated points;
- execution efficiency against stable absolute budgets: 15 advisory points;
- first-pass handoff convergence: 5 advisory points.

Execution efficiency weights total tokens most heavily (6 points), followed by
turns (3), function calls (2), wall time (2), and work amplification (2). Stable
absolute bands make the scenario independently scorable: the best bands are at
most 150k tokens, 28 turns, 30 calls, 180 seconds, and 2x amplification. Missing
or over-budget measurements receive zero for that component, so the Harness
still emits a numeric score instead of hiding the score when no reference case
is present. Wall time remains low-weight because it is more sensitive to runner
and provider noise.

The protected runner installs `scripts/engineering_ticket_fixture.py` as
`/opt/iii-harness-e2e/engineering-ticket-fixture`. Its environment must set:

- `HARNESS_E2E_ENGINEERING_FIXTURE_REPOSITORY` to an absolute local clone or
  bare repository containing the reviewed fixture revision;
- optionally `HARNESS_E2E_ENGINEERING_FIXTURE_ROOT` to a protected lease root
  (the default is `/var/tmp/iii-harness-e2e/engineering-ticket`).

The launcher accepts only these operations:

```text
engineering-ticket-fixture prepare --execution-id <id> --revision <full-sha>
engineering-ticket-fixture cleanup --lease-id <opaque-id>
```

`prepare` creates a clean local shared clone, removes its remote, checks out a
single disposable branch, and configures a local author. It returns JSON with
`path`, `lease_id`, and `revision`. `cleanup` resolves the owned path from the
protected lease record; it never accepts a filesystem path from the workflow.

The workflow exports the returned path as
`HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH`, captures all native Harness
artifacts before cleanup, and removes the lease in an `always()` step.

Release Control campaign groups use the same launcher against the immutable
`tests/fixtures/campaign/engineering-ticket.bundle` shipped by the exact runner
revision. Those common groups stay on an ephemeral GitHub-hosted runner; only
fault injection requires the protected self-hosted runner. The bundle is
cloned offline, has its remote removed, and is cleaned through the same opaque
lease contract.
