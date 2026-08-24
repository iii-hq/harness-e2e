# Engineering ticket Git handoff fixture

`engineering_ticket_git_handoff` v2 and its `engineering_ticket` v2 reference run
against the same disposable, offline Git clone. The campaign workflow never
creates a checkpoint commit: the planner and implementer Harness sessions own
all commits made after the reviewed baseline.

## Verdict and calibrated score

The handoff scenario keeps correctness and protocol integrity as deterministic
hard gates. Passing still requires orchestration discipline, linear Git
checkpoints, ticket acceptance, and bounded scope/lifecycle behavior.

The 100-point score is intentionally more discriminating:

- orchestration discipline: 15 hard-gated points;
- Git handoff integrity: 20 hard-gated points;
- ticket acceptance: 35 hard-gated points;
- scope and lifecycle: 10 hard-gated points;
- efficiency paired with the matching `engineering_ticket` repetition: 15
  advisory points;
- first-pass handoff convergence: 5 advisory points.

Paired efficiency weights total tokens most heavily, followed by turns,
function calls, wall time, and work amplification. Each ratio uses stable bands:
at most 1.25x receives full component credit, followed by 75%, 50%, and 25%
credit through 2.00x; ratios above 2.00x receive zero for that component. Wall
time has a deliberately small weight because it is more sensitive to runner and
provider noise.

Pairing happens after every scenario in the suite has completed and before the
first `results.json` persistence. It requires the same canonical seed, task case
and repetition ordinal. If the matching baseline was not executed, failed, or
lacks complete efficiency evidence, the handoff may still pass its hard gates
but its score is unavailable; missing evidence is never converted into zero.

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
