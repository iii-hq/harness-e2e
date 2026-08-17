# Explainable assessment pipeline

MOT-4446 carries each scenario assessment declaration through execution without
reconstructing its semantics from an awarded score. `CriterionSpec` retains the
stable id, weight, description, assessment kind, hard-gate or advisory policy,
evaluation dimension, and deterministic or judge source. The runner then
materializes exactly one `AssessmentResult` for every declaration, including an
explicit `not_evaluated` or `unavailable` result when execution cannot reach the
assessment.

Results are retained on each attempt and materialized directly into the current
`assessment_contract` in `results.json`.

## Deterministic and judge results

Required checks retain `hard_gate` policy and their declared dimension. Signals
retain `advisory` policy. A failed hard gate remains failed even when it earned
partial points, while the score is preserved as independent evidence. Judge
criteria are always advisory and use the current criterion rubric and prompt.

Successful judge results record provider, model, analyzer name, the canonical
input SHA-256, per-criterion confidence, aggregate attempt
latency, input/output tokens, and cost. Judge execution failures never replace
an already materialized deterministic result. They become scoreless advisory
states with one stable classification prefix:

- `judge_unavailable`;
- `judge_malformed_output`;
- `judge_timeout`; or
- `judge_infrastructure`.

## Qualitative asset review

When a judge is configured, captured assets with both an immutable evidence
reference and a bounded sanitized preview are reviewed as one bounded batch.
The input identifies the current asset-quality rubric, and its canonical hash
is recorded in every analyzer identity. The analyzer can only
score the supplied preview and must return every inspected artifact id and
SHA-256 exactly. Missing, added, or changed evidence identities make the whole
advisory response malformed.

Each qualitative result keeps deterministic validation untouched, uses a
0–100 advisory score and 0–1 confidence, and cites the exact captured evidence.
Assets without immutable content or a bounded preview remain explicitly
`not_evaluated`. If no judge is configured, their qualitative state is
`unavailable`; it is never converted into a zero score or a hard-gate failure.

Before persistence, assessment summaries and asset conclusions pass through
the same redaction policy as the report. Executed criterion results are then
bound to the immutable transcript artifact; qualitative asset results remain
bound to their captured asset artifact.

## Automatic final assessment

After cleanup, the runner first persists the complete objective result and its
redacted evidence. It then creates one `final-assessment-input.json` artifact
for every completed run. That input is capped at 64 KiB and contains scenario
and attempt identity, system status, per-assessment and asset conclusions,
evaluation dimensions, selected numeric metrics, robustness signals, failures,
cleanup outcome, and stable transcript evidence references. Raw transcripts,
scenario prompts, and generated asset contents are never sent to the final
analyzer. Runtime metrics include the durable context-compaction count when the
producer supplied it; legacy absence remains unrecorded rather than being
inferred from the latest context snapshot.

The configured judge analyzes this persisted input and returns an advisory
verdict, 0–100 quality score, confidence, summary, factual observations,
strengths, concerns, recommendation, limitations, and exact evidence
identities. Provider/model identity, canonical input SHA-256, latency, token
usage, and cost are persisted with the result. The input artifact makes the
hash and evidence boundary directly auditable.

Final analysis is automatic even for scenarios whose individual checks are
fully deterministic; when no explicit judge is supplied, the subject
provider/model is used. Unavailable providers, timeouts, malformed output, and
transport failures become explicit advisory availability states after the
objective result has already been persisted. They do not erase the execution
or change `system_status`.

No prompt or payload version is recorded. The current contract is singular;
only `scenario_version` is versioned. Reproducibility comes from scenario
identity, the persisted bounded input, its canonical SHA-256, and the exact
provider/model identity.
