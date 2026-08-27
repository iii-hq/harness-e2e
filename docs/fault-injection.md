# Fault injection and Weekly Stress

Weekly Stress measures one logical `fault_recovery_matrix` scenario from L2
through L4. Its three phases retain separate technical profiles because each
complexity tier has a different subject and amplification budget, but the team
tracks and presents one capability: recover stateful and coordinated work
without corrupting results, duplicating effects, or leaking resources.
Repository code defines and evaluates perturbations, while an environment-owned
supervisor applies them. This boundary prevents code from a source artifact or
pull request from gaining host, network, provider, or storage credentials.

## Evidence contracts

Each attempt has four immutable JSON documents:

1. `FaultProfile` declares the perturbations, expected terminal outcome, seed,
   and maximum acceptable work amplification.
2. `FaultPlan` materializes deterministic action ids and order from the exact
   profile bytes.
3. `FaultJournal` records which actions the protected supervisor actually
   applied, whether recovery was observed, cancellation state, and cleanup
   evidence.
4. `FaultEvaluation` binds the plan and journal to canonical results and assigns
   one outcome: `correct_recovery`, `excessive_recovery`, `incorrect_result`,
   `structural_failure`, or `infrastructure_failure`.

The fault contract supports delay, fail-first, duplicate delivery, child
timeout, transient disconnect, out-of-order results, cancellation, malformed results (the
supervisor corrupts the result payload of the first call to each named target,
so the subject receives undecodable, contract-breaking bytes once), throttle
bursts (the supervisor answers the target's next N calls with a
provider-throttle 429-style rejection at a fixed spacing). A
planned action that was not triggered is a benchmark infrastructure failure;
it cannot be reported as a product failure or a successful recovery.

The recovery matrix has a work-amplification budget calibrated per phase:
`weekly-l2-recovery` (subject family `stateful.2`, amplification ≤ 2.0)
perturbs a childless stateful subject with delay, a failed first write, and
duplicate delivery only — child perturbations would be untriggerable there and
therefore benchmark infrastructure failures; `weekly-l3-recovery`
(`coordination.3`, ≤ 3.0) adds a failed first child, child timeout, and
out-of-order results; `weekly-l3-degraded`
(`coordination.3`, ≤ 2.5) corrupts the first result of a coordination child
with `malformed_result` under a delayed send and duplicate delivery and
expects a degraded finish; `weekly-l4-recovery` (`coordination.4`, ≤ 3.5)
keeps the highest coordinated fault-injection coverage. L5 behavior is covered
by adaptive scenario campaigns, where invalidation and replanning are observed
directly rather than inferred from a synthetic fault profile. The supervisor
owns the mapping from each scenario family to a concrete subject workload.

A `degraded` profile expects the subject to finish WITHOUT a correct
deliverable: a hard-gate-failed but structurally clean, fully cleaned-up,
bounded run. It classifies as `correct_recovery` only when every planned
action was applied and recovered, cleanup completed, structural integrity
held, work amplification stayed within budget, and the canonical results show
the deliverable did not pass; a passed deliverable means the perturbation
failed to bite and is a benchmark infrastructure failure. Structural breakage
stays `structural_failure` and an over-budget run stays `excessive_recovery`.
Degraded profiles must not declare a cancellation rule and require
`--results`. The protected supervisor must implement the `malformed_result`
and `throttle_burst` action kinds before `weekly-l3-degraded` can run; until
it does, a planned-but-untriggered action correctly reports as a benchmark
infrastructure failure rather than a product outcome.

Materialize a plan with:

```bash
cargo run --locked -- fault-plan \
  --profile config/profiles/weekly-l4-recovery.json \
  --output target/fault-plan.json
```

Evaluate the supervisor journal with:

```bash
cargo run --locked -- fault-evaluate \
  --profile config/profiles/weekly-l4-recovery.json \
  --plan target/fault-plan.json \
  --journal target/fault-journal.json \
  --results target/results \
  --output target/fault-evaluation.json
```

Cancellation profiles omit `--results`; recovered and degraded profiles
require it. For cancellation the journal must prove that
`e2e::cancel` reached a terminal cancelled state, the active attempt was
cleared, child work was terminal, and scenario compensation completed.

## Protected supervisor contract

`/opt/iii-harness-e2e/run-weekly-stress` is deployed independently of this
repository and receives the plan as data. It must:

- verify the protected iii `0.23.0-rc.4` binary checksum on every invocation;
- start an empty Engine and a Compose daemon with unique daemon/project
  namespaces and a dedicated state directory;
- run `compose::validate`, `compose::up`, `compose::status`, and
  `compose::down`, persisting the lifecycle responses, process inventories,
  namespaces, and PIDs even on failure;
- reject a profile or plan hash mismatch;
- apply only the bounded actions represented in `FaultPlan`;
- isolate process/network controls to the ephemeral subject stack;
- invoke execution, status, cancellation, and results only through `e2e::*`;
- record actual timestamps and failures in `FaultJournal`;
- compute `results_sha256` as SHA-256 over recursively key-sorted compact JSON,
  prefixed with `sha256:`, matching the runner's canonical value hash;
- verify no active attempt, pending child, namespace, binding, or timer remains;
- restore every proxy, latency rule, process, and temporary resource even when
  the subject, evaluator, or workflow is cancelled.

The source-controlled protected boundary lives in `supervisor/`. Installation
is atomic and requires a separately protected fault driver that answers
`--protocol-check` with `compose-fault-driver`; this prevents activation with
an injector that assumes a different lifecycle. The installer atomically
replaces only the owned `compose-supervisor` bundle, preserves other protected
utilities and separately provisioned secrets, removes retired supervisor state,
downloads only the checksum-pinned iii CLI archive, and installs no auxiliary
lifecycle binary. Provider credentials are root-owned `0600` files in
`/opt/iii-harness-e2e/secrets` and are referenced only through Compose
`env_file` entries.

`/opt/iii-harness-e2e/archive-trusted-execution` attaches the plan, journal,
and evaluation before invoking `e2e::archive`, then invokes
`e2e::archive-head`. The workflow accepts only an available `iii-storage://`
manifest bound to a SHA-256. GitHub Actions artifacts are a short diagnostic
fallback, not longitudinal evidence.

The supervisor implementation and its deployment configuration are protected
environment assets. Changing a repository profile cannot change the
supervisor, expand host access, or expose secrets.

## Failure interpretation

- Correct result, clean lifecycle, and bounded amplification is a successful
  recovery.
- Correct result with amplification above profile policy is an expensive
  recovery and fails the stress gate separately.
- Wrong deliverable is an incorrect result.
- Unrecovered action, incomplete cleanup, or structural dimension failure is a
  structural failure.
- Missing actions, injector errors, mismatched hashes, or unavailable required
  evidence are benchmark infrastructure failures.

The weekly workflow repeats all profiles and continues until both the minimum
sample count and soak duration are satisfied. A matrix failure does not cancel
the other profiles.
