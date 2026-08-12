# E2E cutover, incident response, and rollback

The external path is promoted in this order: pull request, main, daily, then
release. A later lane cannot move while an earlier lane is still legacy or
shadow-only. Every lane publishes results v2 with the `e2e::run` entrypoint,
full source and E2E revisions, execution identity, and an available immutable
`iii-storage://` archive reference.

`policies/cutover-v1.json` is the reviewable policy. Runtime evidence is
validated with:

```bash
python3 scripts/validate_cutover.py \
  --policy policies/cutover-v1.json \
  --evidence cutover-evidence.json \
  --require-stage daily \
  --output cutover-evaluation.json
```

The command is fail-closed. Three distinct equivalent shadow windows are
required before cutover. Release additionally consumes the new path's durable
archive head, not an Actions artifact or mutable dashboard record.

Product workflows call `.github/workflows/canonical-gate.yml` at an immutable
E2E revision that is verified as reachable from the protected default branch.
The reusable workflow checks out that exact revision and downloads candidate evidence as inert data;
an environment-owned resolver then re-reads each archive head through iii and
reconstructs the consumer inventory before the policy validator runs. Candidate
JSON alone can never authorize a cutover or release.

## Incident triage

1. Freeze the affected lane. Do not advance a later lane or delete evidence.
2. Record the source revision, E2E revision, execution id, run id and attempt,
   case ids, seeds, and immutable archive URI.
3. Call `e2e::archive-head`. An unavailable, expired, or hash-mismatched archive
   is an infrastructure incident and must not be classified as a product
   regression.
4. Inspect fault evaluation, deliverable, structural, efficiency, and
   infrastructure dimensions independently. Do not retry hard-gate failures.
5. Retry only a clean bootstrap or storage failure for which no scenario result
   exists. A retry gets a new attempt identity and preserves the failed attempt.
6. If cleanup is incomplete, isolate the ephemeral namespace and run the
   environment-owned compensation procedure before any new attempt.
7. Compare against the immutable promoted baseline only after identity and
   policy eligibility pass.

## Rollback while the legacy window is open

The last operational legacy workflow remains present and disabled for at least
14 days after the release lane cuts over. A protected workflow flag may select
that exact workflow only from its recorded full revision. Rollback does not
change the results-v2 publication contract.

Before legacy removal, execute a drill that:

1. selects the recorded legacy revision;
2. runs one fixed L2 case and one fixed L4 case without production secrets;
3. publishes canonical identity and immutable evidence;
4. restores the new path;
5. confirms both paths leave no active work or temporary resources;
6. records the drill time, outcome, restored workflow, and evidence SHA-256.

The drill, elapsed rollback window, and consumer inventory are mandatory input
to `--require-stage legacy_removal`. The validator rejects removal while an
active consumer still reads results v1 or invokes a legacy binary.

## Rollback after legacy removal

The legacy workflow is retained as a protected immutable tag whose name starts
with `refs/tags/e2e-legacy-`; evidence also records its resolved full Git SHA.
Restore only that tag, rerun the drill, and keep release promotion frozen until
the incident is reconciled. Never point rollback at a branch, `main`, `latest`,
an Actions artifact name, or another mutable reference.

## Removal checklist

- pull request, main, daily, and release are all `new_path`;
- every lane has canonical results v2 and verified durable archive evidence;
- at least three shadow windows met parity policy;
- the 14-day rollback window elapsed;
- the rollback simulation succeeded and is hash-bound;
- the active-consumer inventory contains no results-v1 or legacy entrypoint;
- release protection consumes the external immutable archive;
- the immutable legacy tag and this runbook were verified.

Only then may the old runner, dual results publication, collection fallback,
and legacy workflow be removed in one narrow change.
