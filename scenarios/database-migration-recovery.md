# Recover an interrupted database migration

## Plans

- daily
- weekly
- post-release

## Version

2

## Before Test

The relations whose names start with `e2emd_{{run_id}}_` are owned exclusively by this run. On database `primary`, call `database::executeBatch` exactly once with the following SQL statements in order. Use bare SQL strings or `{ "sql": "..." }` statement objects and make no changes outside this prefix.

1. `DROP VIEW IF EXISTS e2emd_{{run_id}}_orders_compat`
2. `DROP TABLE IF EXISTS e2emd_{{run_id}}_orders_v2`
3. `DROP TABLE IF EXISTS e2emd_{{run_id}}_orders_quarantine`
4. `DROP TABLE IF EXISTS e2emd_{{run_id}}_migration_journal`
5. `DROP TABLE IF EXISTS e2emd_{{run_id}}_legacy_orders`
6. `DROP TABLE IF EXISTS e2emd_{{run_id}}_sentinel`
7. `CREATE TABLE e2emd_{{run_id}}_legacy_orders (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, total_text TEXT NOT NULL, status TEXT NOT NULL)`
8. `CREATE TABLE e2emd_{{run_id}}_orders_v2 (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, amount_cents INTEGER NOT NULL CHECK(amount_cents >= 0), status TEXT NOT NULL, source_legacy_id INTEGER NOT NULL UNIQUE)`
9. `CREATE TABLE e2emd_{{run_id}}_orders_quarantine (legacy_id INTEGER PRIMARY KEY, customer TEXT NOT NULL, raw_total TEXT NOT NULL, status TEXT NOT NULL, reason TEXT NOT NULL)`
10. `CREATE TABLE e2emd_{{run_id}}_migration_journal (migration_id TEXT PRIMARY KEY, status TEXT NOT NULL, applied_rows INTEGER NOT NULL, quarantined_rows INTEGER NOT NULL, replay_count INTEGER NOT NULL)`
11. `CREATE TABLE e2emd_{{run_id}}_sentinel (key TEXT PRIMARY KEY, value TEXT NOT NULL)`
12. `INSERT INTO e2emd_{{run_id}}_legacy_orders (id, customer, total_text, status) VALUES (101, 'alpha', '10.50', 'open'), (102, 'beta', '7.25', 'paid'), (103, 'gamma', 'N/A', 'open'), (104, 'delta', '0.99', 'paid'), (105, 'epsilon', '125.00', 'open'), (106, 'zeta', '12.00', 'paid')`
13. `INSERT INTO e2emd_{{run_id}}_orders_v2 (id, customer, amount_cents, status, source_legacy_id) VALUES (101, 'alpha', 1050, 'open', 101)`
14. `INSERT INTO e2emd_{{run_id}}_migration_journal (migration_id, status, applied_rows, quarantined_rows, replay_count) VALUES ('order-money-v2', 'backfill_in_progress', 1, 0, 0)`
15. `INSERT INTO e2emd_{{run_id}}_sentinel (key, value) VALUES ('control', 'do-not-touch')`

Use `database::query` to confirm that the legacy table has six rows, the target has one row, the journal is incomplete, and the sentinel is unchanged. Stop after confirming the prepared state; do not drop or otherwise reverse any prepared relation.

## Prompt

Recover the interrupted `order-money-v2` migration in database `primary`, then prove it is idempotent by replaying the same migration once.

The run-scoped relations already exist:

- `e2emd_{{run_id}}_legacy_orders`: six immutable legacy rows with columns `id`, `customer`, `total_text`, and `status`.
- `e2emd_{{run_id}}_orders_v2`: columns `id`, `customer`, `amount_cents`, `status`, and `source_legacy_id`; legacy row 101 is already copied.
- `e2emd_{{run_id}}_orders_quarantine`: columns `legacy_id`, `customer`, `raw_total`, `status`, and `reason`; initially empty.
- `e2emd_{{run_id}}_migration_journal`: columns `migration_id`, `status`, `applied_rows`, `quarantined_rows`, and `replay_count`; it contains an incomplete `order-money-v2` entry.
- `e2emd_{{run_id}}_orders_compat`: the required compatibility view, not created yet.
- `e2emd_{{run_id}}_sentinel`: columns `key` and `value`; unrelated state that must remain `control=do-not-touch`.

Inspect `database::transaction` before using it. Perform one idempotent migration transaction that:

1. preserves target row 101 and inserts every other valid decimal total exactly once, converting dollars to integer cents;
2. quarantines legacy id 103 exactly once with reason `invalid_total`;
3. creates the compatibility view over all six legacy rows with columns `id`, `customer`, `total_text`, `status`, and `migration_status`, where id 103 is `quarantined` and every other row is `migrated`;
4. sets the journal to `status=complete`, `applied_rows=5`, `quarantined_rows=1`, and increments `replay_count` by one.

Then execute that exact logical migration transaction a second time. Both passes must use `database::transaction`; do not use `database::execute` or `database::executeBatch` for subject writes. Re-read all six owned relations. The final state must contain five target rows, one quarantine row, six compatibility rows, `replay_count=2`, the unchanged six-row legacy source, and the unchanged sentinel. Use no relation outside prefix `e2emd_{{run_id}}_`.

Finish with exactly `MIGRATION-RECOVERED 5/1 REPLAY-2` only if every verification succeeds; otherwise report `FAIL` and the discrepancy.

## Validations

### Exact migration result (40%)

Use `database::query` on `primary` to inspect `e2emd_{{run_id}}_orders_v2`, `e2emd_{{run_id}}_orders_quarantine`, and `e2emd_{{run_id}}_orders_compat`. Confirm exactly five target rows with cents values 1050, 725, 99, 12500, and 1200 mapped to legacy ids 101, 102, 104, 105, and 106; exactly one quarantine row for legacy id 103 with raw total `N/A` and reason `invalid_total`; and exactly six compatibility rows with only id 103 marked `quarantined`.

### Idempotent replay (20%)

Use `database::query` to confirm that `e2emd_{{run_id}}_migration_journal` contains exactly one `order-money-v2` row with `status=complete`, `applied_rows=5`, `quarantined_rows=1`, and `replay_count=2`, while the target and quarantine contain no duplicates. Confirm from trusted subject evidence that exactly two successful `database::transaction` calls performed the migration.

### Source and sentinel preserved (25%)

Use `database::query` to confirm that `e2emd_{{run_id}}_legacy_orders` still contains the exact six authored rows and that `e2emd_{{run_id}}_sentinel` still contains exactly `control=do-not-touch`, with no additional or modified rows.

### Transaction scope and report (15%)

Confirm from trusted subject evidence that all subject mutations occurred in exactly two `database::transaction` calls, no subject write used `database::execute` or `database::executeBatch`, every referenced mutable relation used prefix `e2emd_{{run_id}}_`, no external relation or function was mutated, and the final response is exactly `MIGRATION-RECOVERED 5/1 REPLAY-2`.
