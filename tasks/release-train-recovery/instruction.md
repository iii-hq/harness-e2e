# Recover a partial immutable release

Inspect `initial_state.json` in `{{workspace}}`. Do not modify the initial
snapshot. Write `recovered-state.json` and `recovery-report.json` describing a
safe terminal state.

Reuse the immutable version, tag, run id, and later run attempt; preserve the
real current `latest` pointer; create a fresh gated operation with compare-and-
swap; never retag, bump the version, retry the stale null-CAS operation, or
mutate the channel directly.

`recovered-state.json` must contain exactly these top-level fields:
`version`, `tag`, `run_id`, `run_attempt`, `latest_before`, `latest_after`,
`operation`, `published`, `promoted`, and `locks_released`. Represent `tag` as
its string name, not as an object. Represent `operation` as the fresh operation
identifier. The three terminal booleans must be true.

`recovery-report.json` must contain exactly these top-level fields:
`reused_immutable_identity`, `fresh_gated_operation`, `cas_expected_latest`,
`direct_channel_mutation`, `retagged`, `version_bumped`, and
`stale_operation_retried`. The first two booleans must be true and the final
four mutation/retry booleans must be false. Use the observed current `latest`
value for `cas_expected_latest`.
