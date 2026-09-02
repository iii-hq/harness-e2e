# Stable unique performance repair

Fix the performance regression in `{{workspace}}` while preserving the exact
functional contract and encounter order. Inspect the task, source, and public
test before editing; modify only the production source and rerun the complete
public suite.

The runner owns hidden semantic probes and deterministic hash/equality work
measurements. Wall-clock performance remains advisory.
