# Result reliability and retained evidence

The execution API remains `e2e::run` v1. The result artifact contract is now
Results v4 (`results.json`); its fingerprint and the scoring-profile fingerprint
identify the evidence contract independently of the API version.

## Partial executions are useful, not successful by default

Every requested slot remains represented, including slots that cannot be
materialized or started. Deferred scenarios carry a `deferral_reason` and planned
counts, without an invented case, run, score, or subject observation. Independent
slots are scheduled round-robin; a local judge failure does not by itself discard
other scenarios. Shared-stack or fixture contamination can still stop unsafe work.

Completion, technical validity, objective outcome, and quality availability are
separate facts. A genuine `task_incomplete` observation is retained as an outcome;
it is not silently retried to improve the score. A cleanup failure makes the
attempt technically invalid even when the subject completed or an earlier gate,
resource-limit, or judge error had already assigned a terminal status.

`report_state: partial` and `persistence_errors` expose an execution whose durable
record could not be completed. Already obtained run evidence stays in the result
when the filesystem still permits writing it. New work stops when recording its
lifecycle cannot be trusted. A total filesystem failure cannot guarantee a final
report; prior immutable checkpoints are the recovery evidence, not a claim of
successful completion. The Console displays persistence errors alongside retained
results instead of showing a clean success.

## Metrics and denominators

Metrics are aggregated per scenario. A rate with a zero denominator is `null`,
not zero. Missing usage for an observed attempt makes the affected total `null`;
the harness does not substitute zero tokens for missing telemetry. Deferred slots
have no invented token usage. Usage from retries is counted once in the final
run's cumulative efficiency record.

| Field | Meaning |
| --- | --- |
| `planned_runs` | All requested slots, including deferred slots. |
| `observed_runs` | Retained runs, excluding slots that never ran. |
| `deferred_runs` | Planned minus observed. |
| `execution_reliability` | Technically valid runs / planned runs. |
| `completion_evidence_coverage` | (`completed_runs` + `task_incomplete_runs`) / planned runs. |
| `completion_rate` | Completed / (`completed` + `task_incomplete`); undetermined runs are excluded and remain visible through coverage. |
| `objective_score_coverage` | Objective-scored runs / planned runs. |
| `quality_score_completed` | Median available quality score among completed runs only. |
| `quality_coverage` | Completed runs with a quality score / completed runs. |
| `total_tokens_consumed` | Subject-side consumption across retained attempts, including failed attempts and retries. |
| `judge_tokens_consumed` | Separately recorded judge usage, including retries. |
| `tokens_completed_p50` | Median cumulative subject tokens among completed runs, including their retry overhead. |
| `failed_attempt_tokens` | Tokens consumed by retry attempts and terminal runs that did not complete, without double-counting. |
| `tokens_per_completion` | Total subject tokens consumed / completed runs. |

For A and C completed with 100k and 120k tokens, and B genuinely incomplete with
20k tokens, completion is 2/3, total consumption is 240k, completed-token p50 is
110k, failed-attempt consumption is 20k, and tokens per completion is 120k.
If B instead has undetermined completion due to infrastructure, completion is
2/2 with only 2/3 completion-evidence coverage. Both numbers must be read together.
Low token consumption on an incomplete or undetermined run is not evidence of
better efficiency.

## Journal and checkpoints

The control-plane execution directory contains `journal/header.json` and
`journal/events/<sequence>-<event>.json`. The immutable header records the request,
its hash, runner identity, result/scoring fingerprints, and admission identity.
Events are ordered and hash-linked. Each candidate event is validated against
the replayed state and referenced artifact hashes **before** being installed;
rejection must not poison subsequent valid appends.

Within the result artifact directory, checkpoints are retained at:

- `journal/observations/<slot_id>/<attempt_id>.json`: recorded subject observation
  and attempt metadata.
- `journal/runs/<slot_id>/<run_id>.json`: full redacted `E2eRunReport`, including
  outcomes, failures, gates, usage, retry records, and evidence references. Its
  `capture` section retains in-memory assessment and deliverable contents that
  the public Results serializer omits, including retry captures.

Full run checkpoints are written independently of journal event acknowledgement,
including CLI runs without a control-plane event sink. A checkpoint alone is not
a committed journal event: recovery and live projections verify event ordering
and referenced bytes before trusting them. Referenced artifacts must be retained
alongside their checkpoint; a checkpoint does not replace their contents.

A rejected persistence checkpoint stops admission of new slots, while capture
and cleanup of the active attempt still run. If final Results persistence fails,
the control plane receives the redacted partial report with explicit errors and
no invented result path. Auxiliary assessment work is skipped if the initial
objective report could not be persisted.

CI uploads include hidden checkpoints only from a validated package. Runtime
state and provider secrets must remain outside the upload tree. If validation
rejects the package, CI uploads a safe failure diagnostic instead of the rejected
contents; it does not silently omit files from the manifest.

Finalized journals are immutable. Restart recovery replays existing evidence and
reports the interruption or reconciliation need; it does not automatically
resume the subject or rerun missing scenarios. `e2e::continue`, `e2e::reevaluate`,
and adaptive automatic restart are not added by this stage. Future continuation
must use a new generation linked to the previous immutable execution, with
explicit identity/CAS checks, rather than appending to a finalized journal.

## Soft slot-start deadline

`slot_start_deadline_seconds` is an optional positive `e2e::run` request field.
If omitted, `HARNESS_E2E_SUITE_DEADLINE_SECONDS` may supply the value. The resolved
value is part of the persisted request and its hash, and is included in the
result/Console projection. With neither configured there is no suite-wide
slot-start limit.

This is a scheduling budget measured from suite start, checked before starting a
new slot. It does **not** interrupt an already running attempt, evaluator,
capture, or cleanup, and is not a hard wall-clock timeout. Slots not started
within the budget remain explicitly deferred. Per-phase hard deadlines are
separate work.

## Historical visibility and comparison

Results v2/v3 are not decoded into v4 metrics. The local Console keeps their
execution identity, timestamps, and subject visible as **unsupported**, using
existing metadata or a minimal validated identity envelope. It does not require
a v4 manifest to list older evidence, alter the old files, invent metrics, or
select those executions as comparison baselines. A malformed JSON document or an
invalid current-version report remains corruption, not a compatibility bypass.

New durable result history is written to `harness_e2e_result_history`.
The legacy `harness_e2e_history` table is deliberately left unmodified for older
readers. There is no automatic backfill, baseline migration, or merging of these
tables into a comparable cohort. Local artifact discovery is distinct from the
new durable-history query API. Retaining an old baseline ID does not make its
result contract compatible; a new baseline must be explicitly selected from
eligible current-contract evidence.
