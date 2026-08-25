# Release policy v3

Status: current
Authority: policy

Every production release must pass admission control before deployment begins. Admission control
requires an immutable build identity, a successful deterministic validation bundle, and an
approved rollout window. A failed admission decision is final for that release attempt; operators
must correct the evidence and create a new attempt instead of retrying the denied mutation.

The rollout starts with a five-percent canary. Promotion beyond the canary requires an observation
window with no critical alerts and an error-budget burn below the documented threshold. Rollback
remains available throughout the observation window.
