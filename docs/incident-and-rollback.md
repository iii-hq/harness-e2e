# E2E promotion and incident response

The canonical path is promoted in order: pull request, main, daily, then
release. Every promoted lane uses `e2e::run`, publishes `results.json`, records
full source and E2E revisions, and retains an available immutable
`iii-storage://` archive reference.

`config/policies/cutover.json` is the reviewable policy. Validate evidence with:

```bash
python3 scripts/validate_cutover.py \
  --policy config/policies/cutover.json \
  --evidence cutover-evidence.json \
  --require-stage daily \
  --output cutover-evaluation.json
```

The command is fail-closed. Three distinct equivalent shadow windows are
required before promotion. The reusable canonical gate checks out an immutable
E2E revision, resolves archive evidence through the environment-owned iii
boundary, and treats downloaded JSON only as an untrusted candidate.

## Incident triage

1. Freeze the affected lane and preserve its evidence.
2. Record source revision, E2E revision, execution and attempt identities, case
   ids, scenario versions, seeds, and immutable archive URI.
3. Call `e2e::archive-head`; unavailable or hash-mismatched evidence is an
   infrastructure incident.
4. Inspect fault, deliverable, structural, efficiency, and infrastructure
   dimensions independently. Do not retry hard-gate failures.
5. Retry only a clean bootstrap or storage failure with no scenario result.
6. Reconcile incomplete cleanup before starting another attempt.
7. Compare completed executions only after identity and scenario-contract
   eligibility pass.

Rollback restores the previous known-good canonical revision and freezes later
lanes. It never selects another payload format or adapter: restored and current
revisions both use `results.json`, and scenario versions remain explicit.
