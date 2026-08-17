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

The automatic final result separates factual observations (`facts`) from
interpretive strengths and concerns, the recommended action, and explicit
limitations. Its cited artifact identities must be an exact subset of the
bounded persisted input. A passing system run can become
`passed_with_concerns`; every objective or technical failure keeps its original
effective status regardless of the AI verdict or quality score.

## On-demand analysis

`AnalysisBundle` is the evidence-grounded input and `AnalysisResponse` binds
facts, interpretations, opportunities, and limitations to its canonical input
SHA-256. Both use recursively sorted compact JSON before SHA-256 calculation.

Scenario contracts are the sole versioned domain: `scenario_version` selects a
materialized scenario definition and remains part of case identity.

## Dashboard projection

The local Dashboard read model joins every scenario run to its exact
`RunAssessmentContract`. Execution details retain the approved per-assessment,
asset-validation, qualitative-review, final-AI, analyzer-usage, and evidence
reference fields. Execution, subject, scenario, test-side, and retained
observation views expose a bounded `assessment_summary` instead of copying
those full results into list responses.

Local and static comparison paths calculate the same scenario contract,
assessment profile, and analyzer profile SHA-256 identities. A comparison is
only `compatible` when all three identities agree within and across both sides
of the selected cohort. Contract, assessment, and analyzer changes or conflicts
are reported independently. The profiles are content identities, not payload
versions; `scenario_version` remains the only versioned domain.

Static publication uses an allowlist projection. It excludes prompts, raw
transcripts, transcript message content, generated-asset previews, and private
artifact paths. It retains bounded assessment conclusions, analyzer
provenance, aggregate metrics, and immutable evidence ids/hashes. Retained
reports without the current assessment contract expose explicit unavailable
assessment state and never synthesize analyzer output.
