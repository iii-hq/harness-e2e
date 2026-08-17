# Durable E2E history

Durable history is an iii-only boundary. Clients call `e2e::*`; the trusted
E2E worker calls `storage::*` and `database::*`. Provider credentials and
database URLs stay in those workers and are never passed to the runner or the
subject under test.

## Contracts

Durable history persists two current records:

- `schemas/durable-archive.json` describes a manifest whose files and
  chunks are content-addressed;
- `schemas/history-record.json` describes one validated analytical row.

Every URI has the form
`iii-storage://<private-alias>/<immutable-key>?sha256=<digest>`. Upload performs
`putObject`, `headObject`, and a hash-verified read-after-write. Restore verifies
the manifest, every chunk, the reconstructed file, and finally the native
`results.json` evidence graph.

The worker-facing aliases are:

| Retention class | Default alias | Lifetime |
|---|---|---:|
| `temporary` | `e2e-temporary` | 1 day |
| `pull_request` | `e2e-pull-request` | 14 days |
| `longitudinal` | `e2e-longitudinal` | 400 days |
| `canonical` | `e2e-canonical` | no automatic expiry |

Deployments may replace an alias with
`HARNESS_E2E_STORAGE_<CLASS>_BUCKET`. `HARNESS_E2E_STORAGE_BACKUP_BUCKET`
selects the manifest-backup alias, and `HARNESS_E2E_HISTORY_DATABASE` selects
the `database` worker connection. Buckets must be private; the local stack
configuration uses function-only local buckets, while production maps the same
aliases to private S3, GCS, or R2 buckets.

## Security and integrity

Before evidence hashes are finalized, results redact configured secret values,
credential-shaped tokens, authorization values, cookies, passwords, and
private keys. Additional environment variables are opt-in through the
comma-separated `HARNESS_E2E_SECRET_ENV_NAMES` list. The archive boundary scans
every file again and fails closed if any artifact still requires redaction.
Binary content is never rewritten implicitly.

The history table stores the complete JSON record and its SHA-256. Ingestion
first reads by deterministic ingestion ID; replay returns a duplicate success
only when the stored record is byte-equivalent. A reused ID with different
content is rejected. `e2e::history-list` validates schema, identity links, and
hash before returning a row to a dashboard or gate.

## Lifecycle and recovery

`e2e::retention-sweep` selects only non-canonical rows whose `expires_at` is at
or before the requested cutoff. Dry-run is the operational default. A real
sweep deletes chunks, the primary manifest, and its backup before tombstoning
the analytical row. Expired references return `expired`; they are not reported
as corrupt results.

Each archive also writes an immutable manifest backup. `e2e::archive-restore`
tries the primary manifest first and the backup second, then reconstructs into
the worker-owned restore directory. It never accepts a caller-selected path.

Rollback keeps the short-lived GitHub Actions artifact and disables calls to
`e2e::archive`. Existing object-store data is not deleted. Re-enable archival,
verify hashes with `e2e::archive-head`, and replay ingestion; deterministic IDs
make the replay idempotent.
