# Migrate persistent state

## Plans

- daily
- weekly

## Version

1

## Before Test

The scope `e2e-markdown-persistent-state` and key `migration_record` are owned exclusively by this scenario. Before the subject runs, use `state::set` to establish this exact baseline value: `{"owner":"quality-suite","revision":1,"status":"pending","items":[{"id":"alpha","completed":true},{"id":"beta","completed":false}],"metadata":{"schema_version":1,"retention":"test-only"}}`. Confirm that the write succeeded. Make no other state changes.

## Prompt

Use `state::get` to read key `migration_record` from scope `e2e-markdown-persistent-state`. Migrate the stored object with exactly one successful `state::set`: preserve `owner`, preserve the existing `alpha` item, mark `beta` as completed, append `{"id":"gamma","completed":true}`, change `revision` to `2`, change `status` to `migrated`, and preserve `metadata` unchanged. Do not write any other scope or key. Then respond with a concise confirmation that includes the new revision and total item count.

## Validations

### Exact migrated state (50%)

Use `state::get` on scope `e2e-markdown-persistent-state` and key `migration_record`. Confirm that the value exactly equals `{"owner":"quality-suite","revision":2,"status":"migrated","items":[{"id":"alpha","completed":true},{"id":"beta","completed":true},{"id":"gamma","completed":true}],"metadata":{"schema_version":1,"retention":"test-only"}}` with no missing or additional fields.

### Read then write once (25%)

Confirm from the trusted subject evidence that it called `state::get` for the owned scope and key before making exactly one successful `state::set` to that same scope and key, with no function-call errors and no writes elsewhere.

### Existing data preserved (15%)

The authoritative baseline values are `owner: "quality-suite"`, the item `{"id":"alpha","completed":true}`, and `metadata: {"schema_version":1,"retention":"test-only"}`. Use `state::get` on the owned scope and key, then confirm that the final state retains those exact baseline values unchanged while completing `beta` and appending `gamma` exactly once.

### Concise migration confirmation (10%)

Confirm that the final response is concise and states that revision `2` now contains exactly `3` items.
