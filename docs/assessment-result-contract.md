# Assessment result contract

MOT-4444 defines the stable data boundary for deterministic checks, captured
asset assessments, and final AI analysis. It does not activate those producers:
normal E2E execution continues to write results schema v2 until the owning
follow-up tickets populate the v3 contract.

## Versioning and compatibility

- `results-v3.json` adds the required top-level `assessment_contract` with
  `contract_version: 1`.
- A v3 write also emits the frozen `results-v2.json` and `results.json`
  projections. Those compatibility files never contain v3 assessment fields.
- Directory reads prefer `results-v3.json`, then `results-v2.json`, then
  `results.json`.
- v1 and v2 readers normalize missing assessment data to explicit
  `unavailable` / `not_evaluated` states. They never infer a result from the
  legacy `passed` field.
- `schemas/results-v2.json` remains immutable. New fields belong in a new
  results schema version or a separately versioned analysis contract.

The checked-in schemas are:

- `schemas/results-v3.json`
- `schemas/analysis-bundle-v1.json`
- `schemas/analysis-response-v1.json`
- `schemas/asset-capture-v1.json`

Rust owns the canonical schema and semantic validation. The Python and
TypeScript compatibility adapters consume the same old/new fixtures so legacy
normalization cannot drift silently between the CLI and dashboard.

## Assessment model

Every run/attempt pair has one `RunAssessmentContract`:

- `system_status` records the technical execution outcome.
- `assessments` records criterion-level deterministic or judge outcomes.
- `assets` keeps deterministic asset validation separate from qualitative
  asset assessment.
- `ai_final_assessment` records availability independently from its optional
  result and analyzer metadata.
- `effective_status` is derived from system and AI states.

Criterion scores use `awarded` and `possible`; `possible` must be positive and
`awarded` cannot exceed it. Final AI quality uses the integer range 0–100.
Confidence uses the range 0–1. An AI-produced result identifies its analyzer,
analyzer version, optional provider/model, prompt version, and canonical input
hash. Evidence references identify an artifact by id and
`sha256:<64 lowercase hexadecimal digits>`, with an optional
locator within that artifact.

Unavailable, failed, and malformed AI executions are first-class states with a
reason. They are not converted into a low score or a technical failure.

Deterministic asset validation distinguishes `valid`, `invalid`, `malformed`,
`oversized`, `not_produced`, `unreadable`, `unsafe_path`, `unexpected`,
`removed_during_cleanup`, and `not_evaluated`. These outcomes describe capture
and validation only; qualitative asset judgment remains separate.

## Status precedence

System failures always win. AI output cannot promote or overwrite
`hard_gate_failed`, `subject_error`, `judge_error`, `resource_limit`, or
`infrastructure_error`.

For a technically passing run:

| Final AI state | Effective status |
| --- | --- |
| pass | `passed` |
| pass with concerns, fail, or inconclusive | `passed_with_concerns` |
| unavailable, malformed, failed, not requested, or not evaluated | `passed` |

`passed_with_concerns` therefore means the system completed successfully but
qualitative analysis found concerns. It is not a replacement for a hard gate.

## On-demand analysis boundary

`AnalysisBundle` v1 is the stable, evidence-grounded input for future manual
analysis. It carries explicit execution/run/attempt identities, assessments,
assets, dimensions, failures, evidence, metrics, excerpts, and limitations.
`AnalysisBundle.input_sha256` identifies the canonical upstream result material
used to assemble the bundle. `AnalysisResponse.input_sha256` binds every
response to the canonical SHA-256 of the complete bundle and separates facts,
interpretations, opportunities, and limitations. Analyzer identity must carry
that same bundle hash.

Both hashes use the repository's canonical JSON hashing rule: object keys are
sorted recursively before compact JSON serialization and SHA-256 calculation.

## Follow-up ownership

This contract deliberately leaves behavior to the tickets that own it:

| Ticket | Responsibility |
| --- | --- |
| MOT-4445 | deterministic asset capture and validation |
| MOT-4446 | per-assessment materialization and qualitative asset assessment |
| MOT-4447 | automatic final AI assessment |
| MOT-4448 | aggregation, transport, publication, and dashboard data bridge |
| MOT-4449 | React presentation |
| MOT-4450 | user-triggered Harness analysis using `AnalysisBundle` / `AnalysisResponse` |
| MOT-4451 | integration, rollout, and end-to-end acceptance |
