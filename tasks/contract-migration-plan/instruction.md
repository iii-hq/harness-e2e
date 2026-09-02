# Plan a compatible contract migration

Read the producer and consumer in `{{workspace}}` without modifying them. Write
`migration-plan.json` describing an ordered migration from v1 to v2 that keeps
the current consumer working throughout rollout.

Include an `evidence` array sorted by path. Each item must name a repository
path and explain which observed contract or consumer behavior supports the
plan.

The plan must explicitly introduce a compatibility response before switching
the consumer, validate both old and new contracts, define a rollback boundary,
and remove compatibility only after adoption evidence. Do not implement code.

Write exactly these top-level fields, with no additional fields:
`objective`, `evidence`, `steps`, `validation`, and `rollback`. Each evidence
item has only `path` and `reason`. Include the contract evidence first, in this
order: `consumer/contract.json`, then `producer/contract.json`; further evidence
may follow in path order.

Represent `objective` and `rollback` as strings. Represent `validation` as an
array of strings, not as nested objects.

Each step has only `id`, `action`, and `completion_signal`. Use these first four
steps in this exact order:

1. `add-v1-v2-compatibility`
2. `validate-dual-contract`
3. `migrate-consumer`
4. `retire-v1-compatibility`
