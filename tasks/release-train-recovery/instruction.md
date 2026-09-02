# Recover a partial immutable release

Inspect `initial_state.json` in `{{workspace}}`. Do not modify the initial
snapshot. Write `recovered-state.json` and `recovery-report.json` describing a
safe terminal state.

Reuse the immutable version, tag, run id, and later run attempt; preserve the
real current `latest` pointer; create a fresh gated operation with compare-and-
swap; never retag, bump the version, retry the stale null-CAS operation, or
mutate the channel directly.
