# Complete a strict sequential pipeline

## Plans

- daily
- weekly

## Version

3

## Before Test

The scope `e2emd-sequential-{{run_id}}` is owned exclusively by this run. Establish the three hidden stage contracts below using exactly one successful `state::set` per key. Do not create any result key during setup.

- Key `stage_ingest`: `{"stage":"ingest","accepts":"SEQ-START-7D1A","issues":"SEQ-INGEST-92B4"}`
- Key `stage_refine`: `{"stage":"refine","accepts":"SEQ-INGEST-92B4","issues":"SEQ-REFINE-C31F"}`
- Key `stage_publish`: `{"stage":"publish","accepts":"SEQ-REFINE-C31F","receipt":"SEQ-RECEIPT-E84D"}`

Use `state::get` to confirm all three contracts. Stop after confirming the prepared state; do not delete a stage key or create any result key. Make no changes outside the owned scope.

## Prompt

Complete a strictly sequential three-stage pipeline in state scope `e2emd-sequential-{{run_id}}`. The stage contracts are hidden from this session and must be read at runtime.

1. Read key `stage_ingest` with `state::get`. Copy its `accepts` and `issues` values into one `state::set` at key `result_ingest` with exact value `{"stage":"ingest","accepted":"<accepts>","issued":"<issues>"}`.
2. Only after that write succeeds, read key `stage_refine`. Verify that its `accepts` equals the `issued` value from the ingest result, then write key `result_refine` with exact value `{"stage":"refine","accepted":"<accepts>","issued":"<issues>"}`.
3. Only after that write succeeds, read key `stage_publish`. Verify that its `accepts` equals the `issued` value from the refine result, then write key `pipeline_receipt` with exact value `{"stage":"publish","accepted":"<accepts>","receipt":"<receipt>"}`.

Call each stage read and result write exactly once, in this order. Make no other state call and do not access another scope. Finish with one short line containing the exact receipt read from the publish contract.

## Validations

### Receipt delivered (40%)

Use `state::get` on keys `stage_publish` and `pipeline_receipt` in scope `e2emd-sequential-{{run_id}}`. Confirm that `pipeline_receipt` exactly copies the publish stage name, accepted token, and receipt, and that the final response contains that exact receipt.

### Exact chain (35%)

Use `state::get` to inspect all three stage contracts and all three result keys. Confirm that ingest accepted the authored start token and issued the token accepted by refine, refine issued the token accepted by publish, and publish produced the stored receipt. Confirm from trusted subject evidence that the calls occurred exactly as `get stage_ingest`, `set result_ingest`, `get stage_refine`, `set result_refine`, `get stage_publish`, `set pipeline_receipt`, with exact values and no reordering or duplicate.

### Execution discipline (15%)

Confirm from trusted subject evidence that there were exactly six non-discovery function calls, all targeted scope `e2emd-sequential-{{run_id}}`, no other function or state key was used, and there were zero function-call errors.

### Concise report (10%)

Confirm that the final response is one short line containing the exact receipt from `stage_publish` and contains no invented token or additional report section.
