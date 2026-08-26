# Insert a database record

## Plans

- daily
- weekly

## Version

1

## Before Test

The `markdown_insert_record` table is owned exclusively by this scenario. Use `database::execute` on the `primary` database to drop it if an interrupted earlier run left it behind, then create it with the columns `id INTEGER PRIMARY KEY AUTOINCREMENT` and `value TEXT`. During cleanup, drop this test-owned table and verify that it no longer exists. Make no other changes.

## Prompt

Use `database::execute` on the `primary` database to add exactly one row to the `markdown_insert_record` table with the text value `harness-e2e-markdown`. Then respond with a short confirmation.

## Validations

### Record created (80%)

Query the `primary` database and confirm that the `markdown_insert_record` table contains exactly one row whose `value` field is `harness-e2e-markdown`.

### Fewer than 10 turns (20%)

Confirm from the trusted metrics of the evaluated session that fewer than 10 turns were used.
