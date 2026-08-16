# Assessment result contract

MOT-4444 defines one current data boundary for deterministic checks, captured
asset assessments, and final AI analysis. Payload schemas are not versioned and
the runner reads and writes only `results.json`. Unknown version fields are
rejected instead of normalized.

The checked-in schemas are `schemas/results.json`,
`schemas/analysis-bundle.json`, `schemas/analysis-response.json`, and
`schemas/asset-capture.json`. Rust owns their structure and semantic validation;
the dashboard consumes the same current assessment contract without a
compatibility adapter.

## Assessment model

Every run/attempt pair has one `RunAssessmentContract`:

- `system_status` records the technical execution outcome;
- `assessments` records criterion-level deterministic or judge outcomes;
- `assets` separates deterministic validation from qualitative assessment;
- `ai_final_assessment` records availability independently from its result;
- `effective_status` is derived from system and AI states.

Criterion scores use `awarded` and `possible`; `possible` must be positive and
`awarded` cannot exceed it. Final AI quality uses 0–100 and confidence uses
0–1. Analyzer identity contains its name, optional provider/model, and the
canonical input hash. Evidence references contain an artifact id, a SHA-256,
and an optional locator.

Unavailable, failed, and malformed AI executions are first-class states. They
are not converted into a low score or a technical failure. System failures
always take precedence; AI output cannot promote or overwrite a hard-gate,
subject, judge, resource-limit, or infrastructure failure.

## On-demand analysis

`AnalysisBundle` is the evidence-grounded input and `AnalysisResponse` binds
facts, interpretations, opportunities, and limitations to its canonical input
SHA-256. Both use recursively sorted compact JSON before SHA-256 calculation.

Scenario contracts are the sole versioned domain: `scenario_version` selects a
materialized scenario definition and remains part of case identity.
