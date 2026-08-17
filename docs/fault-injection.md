# Fault injection and Weekly Stress

Weekly Stress measures real recovery at L4 and L5. Repository code defines and
evaluates perturbations, while an environment-owned supervisor applies them.
This boundary prevents code from a source artifact or pull request from gaining
host, network, provider, or storage credentials.

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

The checked-in profiles exercise delay, fail-first, duplicate delivery, child
timeout, transient disconnect, out-of-order results, and cancellation. A
planned action that was not triggered is a benchmark infrastructure failure;
it cannot be reported as a product failure or a successful recovery.

Materialize a plan with:

```bash
cargo run --locked -- fault-plan \
  --profile config/profiles/weekly-l5-recovery.json \
  --output target/fault-plan.json
```

Evaluate the supervisor journal with:

```bash
cargo run --locked -- fault-evaluate \
  --profile config/profiles/weekly-l5-recovery.json \
  --plan target/fault-plan.json \
  --journal target/fault-journal.json \
  --results target/results \
  --output target/fault-evaluation.json
```

Cancellation profiles omit `--results`; the journal must prove that
`e2e::cancel` reached a terminal cancelled state, the active attempt was
cleared, child work was terminal, and scenario compensation completed.

## Protected supervisor contract

`/opt/iii-harness-e2e/run-weekly-stress` is deployed independently of this
repository and receives the plan as data. It must:

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
