# Incident response composite scenario

> Status: implementation design only. The `incident_response` scenario, its
> fixture worker, assets, gates, and tests described here do not exist yet and
> have not been validated by a live Harness execution.

## Objective

`incident_response` is a Rust-defined composite E2E scenario that measures
whether Harness can investigate, reproduce, remediate, validate, and safely
resolve an isolated software incident.

The subject receives incomplete but sufficient operational evidence. It must
correlate that evidence, choose its own remediation, and work inside a bounded
disposable repository. Deterministic probes, rather than the subject's final
answer or an AI judge, decide whether the incident was handled correctly.

The initial incident is a duplicate payment settlement after an acknowledgement
timeout:

1. revision A is the known-good fixture revision;
2. revision B moves the acknowledgement after a committed side effect and
   removes the effective idempotency boundary;
3. the first delivery commits a settlement, but its acknowledgement times out;
4. redelivery processes the same `event_id` again;
5. the ledger contains two settlements for one order while the service's error
   rate remains deceptively low.

The scenario must never connect to production, use real payment data, deploy a
real service, open a pull request, or publish a change. Every mutation is local
to an environment-prepared fixture and must be compensable.

## What the benchmark measures

The scenario keeps these outcomes independent:

- **deliverable correctness**: the duplicate effect is prevented and the
  required evidence is produced;
- **structural integrity**: investigation precedes mutation, independent
  analyses fan in before diagnosis, and exactly one terminal action occurs;
- **engineering behavior**: the subject reproduces the problem, changes only
  allowed production paths, preserves tests, and reacts to factual validation;
- **operational safety**: a candidate is promoted only after every deterministic
  gate passes; otherwise the known-good revision is restored;
- **efficiency**: turns, tokens, cost, wall time, validation loops, and work
  amplification remain within the materialized contract;
- **runtime reliability**: Harness sessions complete, cancel, and teardown
  without leaked children, triggers, or workspace state;
- **benchmark infrastructure**: fixture, telemetry, deploy simulator, and
  probes are available and internally consistent.

An AI judge may review the usefulness of the diagnosis and incident report, but
all judge results remain advisory.

## Non-goals

- Reusing a real production incident or production credentials.
- Testing a specific remediation implementation such as a unique constraint.
- Prescribing a patch in the prompt or validator feedback.
- Treating a plausible root-cause sentence as proof of diagnosis.
- Allowing an AI output to activate a required workflow node.
- Retrying a partially executed workflow automatically.
- Making the workflow user-editable or loading an executable workflow JSON.
- Implementing the future `incident_response.1` through
  `incident_response.8` daily ladder in the first change.

## Fixture boundary

### Repository

The environment prepares a disposable Git clone and exposes its absolute path
through:

```text
HARNESS_E2E_INCIDENT_FIXTURE_PATH
```

The repository must contain immutable tags or exact SHAs for:

- `known_good`: revision A;
- `incident`: revision B;
- `fixture_contract.json`: repository identity, allowed production paths,
  protected test paths, expected public-test commands, and exact fixture data
  hashes.

The scenario preflight rejects:

- a relative or non-canonical fixture path;
- a missing `.git` directory;
- a dirty worktree;
- unexpected HEAD, tag, remote, or fixture-contract hashes;
- symlinks escaping the repository;
- an unrecognized test or build command;
- a fixture containing known or shape-detected credentials.

The environment, not the scenario repository, owns preparation of the clone and
the fixture worker. This keeps `harness-e2e` independent from product source
code and prevents a candidate subject artifact from gaining host credentials.

### Synthetic data

The fixture has deterministic, non-sensitive records:

- three orders with distinct `order_id` and `event_id` values;
- one incident event, `evt-duplicate-42`;
- one settlement expected for that event;
- two settlements observed after the seeded retry sequence;
- an append-only audit stream;
- bounded logs, metric samples, traces, and a revision diff.

The root cause is represented by effects, not by a magic answer string. The
subject can choose any remediation that satisfies the invariants.

### Fixture worker

An environment-owned worker registers these functions in iii:

| Function | Purpose | Mutability |
| --- | --- | --- |
| `incident-fixture::preflight` | Return exact fixture, revision, contract, and capability identity | Read-only |
| `incident-fixture::baseline` | Snapshot repository, deploy simulator, data, and telemetry hashes | Read-only |
| `incident-fixture::alert` | Submit or reread the synthetic alert with an idempotency key | Idempotent |
| `incident-fixture::reproduce` | Reset the synthetic case and execute the seeded delivery/timeout/redelivery sequence | Compensable |
| `incident-fixture::telemetry` | Return a bounded logs, metrics, trace, or revision-diff slice | Read-only |
| `incident-fixture::validate` | Run public and hidden probes against the candidate workspace | Compensable |
| `incident-fixture::deploy` | Stage, promote, or roll back a local revision in the deploy simulator | Compensable |
| `incident-fixture::reconcile` | Return incident, ledger, audit, deploy, and active-resource state | Read-only |
| `incident-fixture::reset` | Restore the exact initial revision and synthetic state | Compensable |

Every descriptor declares exact request and response schema SHA-256 values.
Function identifiers are constants in Rust and cannot be supplied through node
configuration. Preflight uses `engine::functions::info` to compare the observed
schemas with those hashes before any workflow node executes.

The subject is never allowed to call `incident-fixture::reset`, direct deploy
mutation, or any `e2e::*` function.

## Required generic workflow primitive

The current `harness.prompt@1` step starts an independent session but exposes
only a `completed` output and always grants `*` except `e2e::*`. It also does not
pass a workflow input as Harness filesystem-scope metadata.

Before implementing `incident_response`, add `harness.prompt@2` alongside the
unchanged v1 descriptor and executor. The v2 registration receives a
runtime-owned `HarnessStepPolicy` containing approved filesystem roots and
mandatory function denials. Descriptor-only preflight registers the v2
descriptor without an executable policy.

### New optional input

```text
workspace_root: text_utf8
```

When present, the executor must:

1. require an absolute, canonical UTF-8 path;
2. require it to be inside one of the runtime policy's approved roots;
3. pass `{"fs_scope":{"root":"..."}}` in `SendOptions.metadata`;
4. reject symlink escapes before `harness::send`;
5. retain the normalized path only after redaction.

### Per-node function policy

Add optional code-owned fields to `HarnessStepConfigV2`:

```text
function_allow: [string]
function_deny: [string]
```

Validation must always append these denials regardless of configuration:

```text
e2e::*
incident-fixture::reset
incident-fixture::deploy
```

Investigation nodes receive only read-oriented telemetry, Git/coder read, and
result-file write capabilities. The remediation node may receive bounded coder
and shell capabilities inside `workspace_root`. No node receives a host-global
filesystem scope.

### Structured result convention

The first implementation does not need to trust or route arbitrary JSON parsed
from the final assistant message. Each Harness node writes exactly one declared
JSON result below:

```text
.harness-e2e/<run-id>/<attempt-id>/<node-id>.json
```

The next deterministic node reads that file, applies a checked-in JSON Schema,
verifies provenance against the session transcript, redacts it, captures it as
an asset, and only then makes selected fields available as typed workflow
outputs.

Missing, malformed, oversized, extra, or transcript-inconsistent result files
are hard-gate failures. They are not silently reconstructed from prose.

## Scenario contract

Add `src/scenarios/incident_response.rs` with:

```text
id: incident_response
scenario_version: 1
execution_kind: composite_flow
canonical seed: stable_seed("incident_response")
```

The materialized `ScenarioCase` should use this initial complexity profile:

| Dimension | Value |
| --- | ---: |
| planning depth | 6 |
| dependency depth | 8 |
| parallel branches | 3 |
| external systems | 4 |
| state transitions | 14 |
| wake cycles | 0 |
| validation loops | 2 |
| artifact count | 11 |
| coordination edges | 16 |
| ambiguity level | 7 |

This derives `L5Adaptive`. Required capabilities are:

```text
e2e::control-plane-v1
harness::independent_session
iii::functions
iii::coder
iii::shell
incident_fixture::v1
```

`ScenarioCase.inputs` contains only stable, non-secret case data:

- incident variant and fixture contract hash;
- known-good and incident revision identities;
- incident event id;
- expected invariant ids;
- allowed and protected path patterns;
- public probe ids and hidden-probe manifest hash;
- maximum repair rounds;
- workflow resource budgets.

The prompt stored on `ScenarioSpec` describes the scenario purpose only. The
composite driver sends node-specific prompts to Harness.

## Workflow definition

Add a code-owned module at:

```text
src/workflow/incident_response/
  mod.rs
  definition.rs
  executor.rs
  evaluation.rs
  fixture.rs
  helpers.rs
  prompts.rs
  schemas.rs
  tests.rs
```

The `WorkflowDefinitionV1` uses:

```text
schema_version: 1
id: incident_response
scenario_version: 1
max_parallel: 3
max_nodes: 20
step_timeout_seconds: 600
workflow_timeout_seconds: 3600
max_total_tokens: 900000
max_cost_usd: 30.00
technical_retries: 0
```

Technical retries remain zero because Harness sessions and deploy operations
are not entirely idempotent. Repetition happens as a fresh attempt after the
mandatory cleanup has proved restoration.

### Node graph

| Node | Step type | Required | Dependencies | Responsibility |
| --- | --- | --- | --- | --- |
| `preflight_fixture` | `incident_response.preflight_fixture@1` | yes | none | Verify contracts, path, revisions, cleanliness, and fixture worker |
| `capture_baseline` | `incident_response.capture_baseline@1` | yes | preflight | Capture immutable pre-incident state |
| `deduplicate_alert` | `incident_response.deduplicate_alert@1` | yes | baseline | Submit the same alert twice and prove one incident identity |
| `reproduce_incident` | `incident_response.reproduce_incident@1` | yes | alert | Execute the seeded timeout/redelivery and prove the duplicate effect |
| `analyze_logs` | `harness.prompt@2` | yes | reproduce | Produce a read-only log hypothesis |
| `analyze_metrics` | `harness.prompt@2` | yes | reproduce | Produce a read-only metric hypothesis |
| `analyze_trace_change` | `harness.prompt@2` | yes | reproduce | Correlate trace and revision diff |
| `validate_triage` | `incident_response.validate_triage@1` | yes | three analyses | Validate files, provenance, read-only discipline, and parallel fan-in |
| `synthesize_diagnosis` | `harness.prompt@2` | yes | validate triage | Rank hypotheses and select falsification probes |
| `validate_diagnosis` | `incident_response.validate_diagnosis@1` | yes | synthesis | Check evidence grounding and emit deterministic `ready_for_remediation` |
| `apply_remediation` | `harness.prompt@2` | no | validate diagnosis | Apply a subject-chosen bounded patch when reproduction and diagnosis are valid |
| `validate_candidate` | `incident_response.validate_candidate@1` | no | remediation terminal | Capture patch and execute public, hidden, and canary probes |
| `decide_terminal_action` | `incident_response.decide_terminal_action@1` | yes | diagnosis, candidate terminal | Emit deterministic and mutually exclusive `should_promote`/`should_rollback` |
| `promote_candidate` | `incident_response.promote_candidate@1` | no | decision | Promote only the exact validated candidate SHA |
| `rollback_candidate` | `incident_response.rollback_candidate@1` | no | decision | Restore exact known-good revision and state |
| `reconcile_final_state` | `incident_response.reconcile_final_state@1` | yes | promote/rollback terminal | Verify deploy, ledger, incident, audit, and active resources |
| `write_incident_report` | `harness.prompt@2` | yes | reconciliation | Produce the final bounded human-readable report |
| `validate_incident_report` | `incident_response.validate_incident_report@1` | yes | report | Validate factual references; publish advisory report quality |

`apply_remediation`, `validate_candidate`, `promote_candidate`, and
`rollback_candidate` are optional because conditional nodes cannot be required.
All deterministic criteria are produced by required validation or decision
nodes.

`validate_candidate` uses `DependencyPolicy::Terminal` so a failed remediation
still becomes deterministic rollback evidence. `decide_terminal_action` reads
attempt-owned fixture state rather than binding a required input to an optional
output.

### Activation rules

- `apply_remediation` activates only when
  `validate_diagnosis.ready_for_remediation == true`.
- `validate_candidate` uses the same deterministic condition after the
  remediation node reaches a terminal state.
- `promote_candidate` activates only when
  `decide_terminal_action.should_promote == true`.
- `rollback_candidate` activates only when
  `decide_terminal_action.should_rollback == true`.
- The decision evaluator must reject both booleans being equal.

No required node is activated from `ControlSource::Ai`.

## Harness prompts

Prompts are Rust constants assembled only from bounded materialized inputs and
typed ancestor data. All fixture data is wrapped as untrusted input.

### Investigation prompts

Each investigation session must:

- inspect only its assigned evidence surface;
- make no repository, database, deploy, trigger, or fixture mutation;
- return one or more hypotheses;
- cite exact evidence ids and observations;
- state a probe that could falsify each hypothesis;
- write its result JSON to the declared path;
- avoid prescribing work to the other investigation sessions.

The result schema contains:

```json
{
  "analysis_kind": "logs|metrics|trace_change",
  "hypotheses": [
    {
      "id": "stable-id",
      "claim": "bounded text",
      "evidence_ids": ["bounded-id"],
      "observations": ["bounded text"],
      "falsification_probe": "bounded text",
      "confidence": 0.0
    }
  ],
  "limitations": ["bounded text"]
}
```

Confidence and prose are advisory. Evidence identity, result shape, read-only
discipline, and session structure are deterministic.

### Diagnosis prompt

The synthesis session receives the three sanitized analysis assets and the
reproduction record. It must produce:

- ranked hypothesis ids;
- a selected root-cause hypothesis;
- evidence ids supporting and contradicting it;
- explicit causal chain from delivery to duplicated effect;
- one or more deterministic probes to run before mutation;
- remaining uncertainty and limitations.

The prompt must not contain the expected root-cause answer or a required patch.

### Remediation prompt

The remediation session receives only:

- validated diagnosis asset;
- allowed and protected path policy;
- public build/test commands;
- maximum repair rounds;
- exact workspace root.

It must inspect the implementation, choose the smallest sufficient production
change, run focused public tests, and write a remediation summary. It may not
modify tests, fixture contracts, hidden probes, E2E evidence, or Git metadata.

Validator feedback reports facts only. Example:

```text
probe concurrent_duplicate: event evt-duplicate-42 produced 2 settlements;
expected 1. Protected files were unchanged.
```

It must not say how to implement idempotency.

### Incident report prompt

The final session receives only validated, sanitized asset summaries and exact
evidence references. It produces a Markdown report containing:

- impact;
- timeline;
- observed cause and limitations;
- remediation or rollback action;
- validation performed;
- residual risk;
- evidence ids.

Report usefulness is advisory. Incorrect artifact ids, revision ids, terminal
action, or validation claims are deterministic failures.

## Assets

All assets are captured before cleanup, redacted before persistence, bounded by
the workflow asset limits, and referenced by SHA-256.

| Asset id | Producer node | Kind / media type | Required content |
| --- | --- | --- | --- |
| `baseline_snapshot` | capture baseline | `incident_baseline` / JSON | revisions, contract hashes, clean status, deploy/data/telemetry hashes |
| `incident_record` | deduplicate alert | `incident_record` / JSON | alert fingerprint, request count, one incident id, dedup evidence |
| `reproduction_record` | reproduce incident | `incident_reproduction` / JSON | event, attempts, timeout point, expected/observed settlement counts, ledger delta |
| `triage_bundle` | validate triage | `incident_triage` / JSON | three validated analyses, evidence refs, session provenance, parallel timing |
| `diagnosis_record` | validate diagnosis | `incident_diagnosis` / JSON | selected hypothesis, causal chain, supporting/contradicting evidence, probes |
| `remediation_patch` | validate candidate | `code_patch` / `text/x-diff; charset=utf-8` | exact bounded diff against incident revision |
| `change_manifest` | validate candidate | `change_manifest` / JSON | before/after hashes, changed paths, protected-path proof, candidate SHA |
| `validation_matrix` | validate candidate | `validation_matrix` / JSON | public, hidden, regression, concurrency, reconnect, and canary results |
| `decision_record` | terminal decision | `incident_decision` / JSON | gate vector, promote/rollback booleans, exact target revisions, reason |
| `final_state` | reconciliation | `incident_final_state` / JSON | deployed revision, ledger, audit, incident status, active resources |
| `incident_report` | validate report | `incident_report` / `text/markdown; charset=utf-8` | bounded final report with valid evidence references |

The runner also retains workflow checkpoints, per-step outputs, transcripts,
metrics, costs, failures, cleanup state, the flow snapshot, final assessment
input, and canonical `results.json`. Those are execution evidence, not
substitutes for the domain assets above.

## Deterministic gates

### Preflight and baseline

- `fixture_contract_exact`
- `fixture_revision_exact`
- `fixture_clean_before_run`
- `worker_contracts_exact`
- `baseline_captured_before_mutation`
- `baseline_artifact_complete`

### Alert and reproduction

- `alert_deduplicated`
- `incident_identity_stable`
- `incident_reproduced`
- `duplicate_effect_observed`
- `reproduction_repeatable`
- `reproduction_provenance_complete`

Failure to apply the planned fixture action, contradictory fixture evidence, or
unavailable telemetry is an infrastructure failure. A consistent fixture that
the subject fails to understand is not.

### Triage and diagnosis

- `three_investigations_completed`
- `investigations_parallel`
- `investigations_read_only`
- `triage_results_schema_valid`
- `triage_evidence_references_valid`
- `fan_in_after_all_analyses`
- `diagnosis_schema_valid`
- `diagnosis_grounded_in_reproduction`
- `falsification_probe_executed`
- `diagnosis_precedes_mutation`

The evaluator checks causal consequences and independent probes. It does not
require a particular phrase such as "missing idempotency".

### Remediation and validation

- `patch_produced`
- `allowed_paths_only`
- `protected_paths_unchanged`
- `tests_unchanged`
- `fixture_contract_unchanged`
- `working_tree_contains_only_candidate_change`
- `focused_tests_passed`
- `duplicate_delivery_safe`
- `concurrent_duplicate_safe`
- `ack_timeout_replay_safe`
- `distinct_events_preserved`
- `ledger_invariant_restored`
- `audit_history_preserved`
- `full_regression_passed`
- `canary_budget_passed`
- `repair_round_budget_passed`

Changing an expected value, deleting duplicate rows without auditable
compensation, weakening an assertion, or bypassing the retry path fails a hard
gate even if the public test command exits successfully.

### Decision and reconciliation

- `terminal_decision_exclusive`
- `promotion_requires_all_gates`
- `promotion_uses_validated_sha`
- `rollback_uses_known_good_sha`
- `one_terminal_operation`
- `deployed_revision_reconciled`
- `ledger_reconciled`
- `incident_status_reconciled`
- `no_active_fixture_operation`
- `no_leaked_harness_session`
- `evidence_captured_before_cleanup`
- `fixture_restored_after_cleanup`

Correct remediation followed by failed cleanup is not a passing run.

## Assessments

Workflow criterion weights total 100 and are produced only by required,
deterministic validation nodes:

| Criterion | Weight | Policy | Producer |
| --- | ---: | --- | --- |
| `incident_reproduction` | 15 | hard gate | reproduce incident |
| `evidence_grounded_diagnosis` | 20 | hard gate | validate diagnosis |
| `remediation_integrity` | 25 | hard gate | terminal decision, using candidate validation state |
| `safe_terminal_action` | 25 | hard gate | terminal decision |
| `final_reconciliation` | 15 | hard gate | reconcile final state |

Qualitative asset review may score the diagnosis and incident report, but it
must cite exact bounded asset hashes and cannot change the system outcome.

## Attempt-owned state and cleanup

`IncidentFixtureState`, shared only inside one materialized runtime, records:

- canonical fixture path;
- initial HEAD and known-good/incident SHAs;
- initial worktree status and data hash;
- incident id and alert fingerprint;
- reproduction run id;
- candidate SHA and changed paths;
- validation result;
- terminal action and deployed revision;
- active Harness sessions and fixture operations.

The mandatory cleanup hook is idempotent and always attempts, in order:

1. stop active fixture operations;
2. stop and teardown remaining Harness sessions;
3. restore deploy simulator to its initial revision;
4. restore synthetic database and telemetry snapshot;
5. reset the fixture to the exact initial HEAD;
6. remove attempt-owned result files and temporary branches;
7. verify a clean worktree and zero active resources.

Cleanup must never delete persisted E2E assets. Its reconciliation result binds
the pre-cleanup asset capture by immutable reference.

If execution and cleanup both fail, retain both failures. Never report the
fixture as restored without the final deterministic checks.

## Failure classification

| Condition | Classification |
| --- | --- |
| Wrong patch, modified tests, invalid diagnosis evidence, unsafe promotion, or leaked domain state | hard-gate failure |
| Subject session cannot use available functions or terminates without its required result | subject error |
| Token, cost, turn, stuck, or validation-loop budget exceeded | resource limit |
| Judge unavailable or malformed | advisory judge state |
| Fixture worker unavailable, schema mismatch, planned reproduction not injected, contradictory fixture evidence, or corrupt baseline | infrastructure error |
| Mandatory cleanup fails after a completed workflow | technical/infrastructure failure with cleanup phase |

The current system run status has no `inconclusive` variant. An AI final
assessment may return an advisory `inconclusive` verdict, but failure of this
deterministic fixture to materialize its declared incident is an infrastructure
error rather than a low subject score.

The aggregate `E2E suite failed` message is not the diagnosis. `results.json`,
semantic step reports, hard gates, failure phases, and cleanup state remain
authoritative.

## Code registration changes

Implementation requires these narrow registrations:

1. Add `pub mod incident_response` and a new `ScenarioId::IncidentResponse` in
   `src/scenarios/mod.rs`.
2. Register `incident_response::scenario` and `materialize` in the existing
   `ScenarioId` matches.
3. Return `ScenarioExecutionKind::CompositeFlow`.
4. Add `pub mod incident_response` in `src/workflow/mod.rs`.
5. Extend `composite_definition`, `composite_descriptor_catalog`, and
   `composite_runtime` with the new definition, descriptor catalog, runtime,
   and cleanup hook.
6. Register `harness.prompt@2` with the filesystem and function-policy boundary
   described above while preserving `harness.prompt@1` unchanged.
7. Do not add a separate runner, results schema, dashboard route, or executable
   workflow file.

The generic dashboard semantic-test projection should render the new workflow
without scenario-specific frontend code. UI changes are justified only if a
current generic field is missing or incorrectly projected.

## Test plan

### Unit tests

- All step descriptors validate and contain no configurable function ids.
- Observed fixture function schemas must match exact hashes.
- The complete definition validates against the descriptor-only catalog.
- The graph has the expected nodes, maximum parallelism, dependencies,
  terminal policies, and activation conditions.
- Criterion weights total 100 and originate from required deterministic nodes.
- Decision truth table always selects exactly one of promote or rollback.
- Promotion is impossible for every individual failed hard gate.
- Reproduction evaluator distinguishes subject failure from missing fixture
  action.
- Patch evaluator rejects protected paths, test changes, Git metadata, symlink
  escapes, oversized diffs, and fixture-contract changes.
- Triage evaluator rejects fabricated evidence ids and mutation during read-only
  sessions.
- Result schemas reject unknown fields, oversized text, extra files, and
  inconsistent transcript provenance.
- Cleanup is idempotent from every partial `IncidentFixtureState`.
- Redaction removes secrets before hashing, previewing, or persistence.

### Scheduler tests with fake executors

- Happy path activates remediation, candidate validation, and promotion.
- Failed diagnosis skips remediation and selects rollback.
- Remediation technical failure still reaches deterministic rollback.
- Hidden validation failure selects rollback and never promotion.
- Three analysis nodes overlap in time and fan in before synthesis.
- Cancellation calls step cancellation, then mandatory cleanup.
- A checkpoint never reports both terminal actions active or completed.
- A skipped optional node is retained with an explicit skip reason.

### Fixture integration tests

- Revision B reproducibly creates the duplicate settlement.
- Revision A does not reproduce it.
- Public tests alone do not cover every hidden invariant.
- Hidden probes cover sequential duplicate, concurrent duplicate, timeout after
  side effect, reconnect replay, and distinct-event behavior.
- Deploy simulator promotes and rolls back only exact known SHAs.
- Reconciliation detects mismatched ledger, audit, incident, and deploy state.
- Reset restores byte-identical data and a clean Git tree.

### Live validation

After unit and integration validation, run one catalog/preflight execution and
one real Harness execution against a disposable local fixture. Do not describe
the scenario as operationally adopted from compile or unit-test evidence alone.

The initial manual command will be:

```bash
HARNESS_E2E_INCIDENT_FIXTURE_PATH=/absolute/path/to/disposable-fixture \
  cargo run --locked --bin harness-e2e -- run \
  --url ws://127.0.0.1:49134 \
  --model <subject-model> \
  --provider <subject-provider> \
  --scenario incident_response \
  --technical-retries 0 \
  --runs-dir target/incident-response-runs
```

For longitudinal use, first preserve one immutable scenario version, seed,
model, judge, lane, stack mode, fixture contract, and E2E revision. Run five
repetitions for repeatable evidence and reserve 20-repetition p95 claims for a
later soak after the scenario itself is validated.

## Implementation sequence

1. Extend and test the generic `harness.prompt` filesystem and function-policy
   boundary.
2. Specify and implement the external fixture repository and environment-owned
   worker contracts.
3. Add the scenario case, complexity, deliverable contract, and catalog
   registration.
4. Add the Rust workflow definition and descriptor-only preflight.
5. Implement fixture state, deterministic executors, evaluators, and cleanup.
6. Add bounded Harness prompts and result schemas.
7. Complete unit, scheduler, fixture integration, and schema tests.
8. Run the real local scenario and inspect `results.json`, assets, semantic
   tests, session lifecycle, and cleanup evidence.
9. Only after stable live evidence, derive the incremental daily rungs without
   changing the original contract.

## Definition of done

The scenario is implemented only when all of the following are true:

- the complete code-owned workflow validates against its registered catalog;
- fixture contracts and immutable revisions are verified before execution;
- all expected assets are schema-valid, redacted, hashed, and captured before
  cleanup;
- every hard gate has a deterministic evaluator and evidence reference;
- both promotion and rollback paths have automated tests;
- cancellation and partial failure prove mandatory cleanup;
- one real Harness execution preserves an inspectable report and assets;
- no test result, AI judge result, or final response can override a failed hard
  gate;
- no production service, credential, repository, or deployment is touched.
