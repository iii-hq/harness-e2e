# Plan a compatible contract migration

Read the producer and consumer in `{{workspace}}` without modifying them. Write
`migration-plan.json` describing an ordered migration from v1 to v2 that keeps
the current consumer working throughout rollout.

The plan must explicitly introduce a compatibility response before switching
the consumer, validate both old and new contracts, define a rollback boundary,
and remove compatibility only after adoption evidence. Do not implement code.
