# Reconciliation repair

Repair the event reconciliation implementation in the isolated workspace
`{{workspace}}`.

Inspect the task, source, and public test before editing. Reproduce the public
failure, record a diagnosis, modify only the production source, and rerun the
complete public suite. Run the repaired CLI in the host workspace and preserve
its exact output contract.

Tests and task metadata are protected. Hidden runner-owned probes cover
out-of-order revisions, account movement, conflicting events, validation,
idempotency, and input immutability.
