# Durable asset evidence

MOT-4445 turns scenario deliverables into bounded, deterministic evidence. The
capture pipeline runs after the Harness session reaches a terminal state and
before teardown or scenario cleanup. It does not call a judge or perform a
qualitative review.

## Lifecycle

For every attempt with a deliverable contract, the runner:

1. inventories declared and captured asset ids;
2. rejects unsafe ids before deriving a filesystem path;
3. applies per-asset and aggregate byte/count limits;
4. redacts sensitive JSON values and provenance strings;
5. validates kind, JSON Schema, provenance, and declared invariants;
6. writes bounded asset content under
   `deliverables/<run-id>/<attempt-id>/<asset-id>.json`;
7. writes `evidence/<run-id>/<attempt-id>/asset-capture-v1.json`; and
8. only then enters teardown and scenario cleanup; then
9. writes `asset-reconciliation-v1.json` beside the capture manifest with the
   final lifecycle state and an immutable reference to the pre-cleanup capture.

The manifest follows `schemas/asset-capture-v1.json`. It embeds the canonical
`AssetValidationResult` from the v3 contract and records the run and attempt
identity, capture timestamp, active limits, expected/unexpected state,
normalized evidence path, media type, observed size, canonical content hash,
immutable artifact reference, provenance, bounded preview, validation outcome,
whether the preview was truncated, and whether cleanup reconciliation ran.

If cleanup removes an already captured artifact, the final assessment records
`removed_during_cleanup`. Final report materialization restores the bounded,
sanitized content and verifies its immutable reference so evidence remains
inspectable. The primary report references the reconciliation manifest, which
in turn binds the exact immutable pre-cleanup manifest; neither lifecycle phase
is overwritten.

## Limits and safety

The default capture budget is:

- at most 64 captured assets per attempt;
- at most 16 MiB across captured assets per attempt;
- at most 1,024 bytes of preview per asset; and
- the smaller of the global byte budget and each scenario asset's
  `max_size_bytes` for an individual asset.

Only non-empty ASCII alphanumeric ids plus `-` and `_` can become artifact
paths. IDs containing a known or shape-detected secret are redacted in the
inventory and rejected before path derivation. Unsafe ids, over-limit content,
and entries beyond the inventory limit are never persisted. Deterministic
type/schema checks use the original bounded value; JSON fields and values
matching the repository redaction policy are sanitized before hashing,
previewing, or writing. `observed_size_bytes` records the bounded original
encoded size so redaction cannot hide a limit violation.

## Structured outcomes

Capture produces one deterministic `AssetValidationResult` per observed or
expected asset. Outcomes distinguish:

- `valid`;
- `invalid` invariant or provenance evidence;
- `malformed` kind or schema content;
- `oversized` per-asset, aggregate, or inventory limits;
- `not_produced` expected assets;
- `unreadable` capture failures;
- `unsafe_path` rejected ids;
- `unexpected` undeclared assets; and
- `removed_during_cleanup` lifecycle loss.

Successful and safely malformed/unexpected assets cite the exact immutable
artifact id and SHA-256. Rejected or absent content carries no fabricated
evidence reference. The adjacent qualitative asset assessment remains
`not_evaluated` in memory; MOT-4446 owns that producer. The canonical primary
result remains v2 until MOT-4448 aggregates these sidecars and activates v3
publication.
