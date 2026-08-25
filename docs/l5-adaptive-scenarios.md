# L5 adaptive scenarios

L5 is a capability classification, not a synonym for a long prompt or a large
token budget. A capability-v2 L5 case must let the subject decompose work, must
invalidate material evidence during execution, and must require a bounded
replan. It must also exercise at least two of: multiple external systems,
parallel branches, compensable mutations, durable resume, or a coherent long
horizon.

Human horizon, execution realism, shadow availability, and resource envelopes
are reported separately. Historical results keep their legacy-v1 classifier;
new results use the capability-v2 classifier and are written as results schema
v3.

## Trusted adaptive boundary

`AdaptiveFlow` accepts a structured plan containing only registered template
ids, node ids, dependencies, and bounded instructions. The runner owns step
types and versions, function policy, filesystem roots, activation controls,
criteria, resource limits, mutation authorization, and terminal anchors.

The first plan is frozen before execution. A second and final revision is
accepted only when it names the previous plan hash and cites evidence from a
material invalidation. Completed work and terminal anchors cannot be replaced.
The materialized result is an ordinary validated `WorkflowDefinitionV1`.

The public workflow checkpoint remains evidence-only. Resume uses a separate
runner-owned `WorkflowResumeStateV1`, written atomically and bound to execution,
scenario, policy, plan, workflow, runtime, and artifact hashes. An uncertain
side effect or identity mismatch becomes `needs_reconciliation`; it is never
blindly retried. Explicit cancellation compensates and does not resume.

## Canonical cases

### `incident_response`

The subject proposes an initial investigation. A deterministic probe disproves
the plausible provider-duplicate hypothesis: distinct event ids do not
reproduce, while redelivery of the same id after an ACK timeout does. The
subject must cite that evidence in its second plan before remediation.
Promotion and rollback stay mutually exclusive and runner-controlled.

Envelope: 60–120 human minutes, 3,600 execution seconds, 750,000 subject
tokens, reported cost up to USD 25, 20 nodes, parallelism 3, two plan revisions.

### `release_train_recovery`

The local simulator starts with an immutable tag and a cancelled attempt after
partial assets. The correct path reruns the same run id, completes attempt 2,
and verifies exact-version publication. A later preview exposes an incompatible
historical `latest` graph and a stale operation with a null CAS expectation.
The replan must preserve the real pointer and create one fresh gated operation;
retagging, version bumps, stale-operation retry, direct `latest` mutation, early
promotion, and secret leakage fail deterministic gates.

An optional environment-owned snapshot may compare read-only GitHub, Registry,
Release Control, and Workers metadata. Shadow absence or disagreement is
advisory and cannot change the simulator's objective result.

Envelope: 120–240 human minutes, 7,200 execution seconds, 900,000 subject
tokens, reported cost up to USD 30, 24 nodes, parallelism 3, two revisions.

### `cross_repo_contract_migration`

The fixture creates three deterministic Git repositories. Initially only the
producer and consumer A are visible. A trusted canary later reveals consumer B
and its compatibility failure, forcing a second plan. The final gates require
old and new clients, the producer contract, all three test suites, allowed-path
integrity, clean provenance, no network use, one terminal rollout, and cleanup.

Envelope: 90–180 human minutes, 5,400 execution seconds, 700,000 subject
tokens, reported cost up to USD 25, 20 nodes, parallelism 3, two revisions.

## Campaign admission and calibration

The three adaptive cases run as separate, single-run, zero-retry groups in the
post-release and weekly advisory campaigns. `moving_target`, now an L2 case,
runs daily as a cheap invalidation precursor. Campaign manifests do not select
or rotate seeds.

A deterministic reference run establishes `reference_verified`. Five
compatible samples establish `repeatable`; twenty establish
`tail_calibrated`. Admission is immediate, but a single run never claims
statistical robustness and quality signals never block promotion.
