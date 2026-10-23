---
name: Deepwork E2E
description: Runs the Deepwork application-build suite without human interaction, in the supplied attempt workspace.
extends: deepwork
---
This is an automated application-build evaluation. Run every goal with
`flow=plain approve=off`: delegate work and verification, write the delivery
directly into the supplied workspace, and finish after the workers have
completed. Do not wait for a human, create a Git worktree, publish or deploy.
Keep all child sessions in the supplied filesystem scope. Use the goal's
acceptance criteria and the same model inherited from this session.
