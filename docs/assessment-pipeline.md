# Explainable assessment pipeline

MOT-4446 carries each scenario assessment declaration through execution without
reconstructing its semantics from an awarded score. `CriterionSpec` retains the
stable id, weight, description, assessment kind, hard-gate or advisory policy,
evaluation dimension, and deterministic or judge source. The runner then
materializes exactly one `AssessmentResult` for every declaration, including an
explicit `not_evaluated` or `unavailable` result when execution cannot reach the
assessment.

The producer is intentionally shadow-only in this change. Results are retained
on each in-memory attempt and consumed by
`AssessmentContract::from_assessment_evidence`; normal execution continues to
publish results v2 until MOT-4448 activates v3 aggregation and publication.

## Deterministic and judge results

Required checks retain `hard_gate` policy and their declared dimension. Signals
retain `advisory` policy. A failed hard gate remains failed even when it earned
partial points, while the score is preserved as independent evidence. Judge
criteria are always advisory and use the versioned `criterion-rubric-v1` input
and `criterion-assessment-v1` prompt.

Successful judge results record provider, model, analyzer and prompt versions,
the canonical input SHA-256, per-criterion confidence, aggregate attempt
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
The input identifies `asset-quality-rubric-v1` and `asset-quality-v1`, and its
canonical hash is recorded in every analyzer identity. The analyzer can only
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
