# Test 3 — Prepare a reproducible Registry environment

Work in `registry/`, pinned to the required commit. Read `inputs/requirements.md`, the public seed and artifacts under `inputs/`, and `inputs/environment.json`. Build the development environment needed to install, migrate, seed, start, and verify PostgreSQL/pgvector, the Registry API, and the web application.

Create the required Dockerfile, Compose configuration, and minimal scripts yourself. No prepared Dockerfile, Compose file, or smoke script is supplied. Pin runtime versions, keep each attempt isolated, use the supplied deterministic seed and local artifacts, and document one-command startup, verification, logs, and teardown. Verify the pinned Registry SHA, database migrations, seeded versions, API health, local artifact downloads and hashes, and real browser access to the existing Registry UI. Do not implement the comparison feature or edit its product behavior.

Leave the environment implementation in `registry/`. Write `output/report.md` in English with architecture, exact commands and outcomes, image/runtime identities, URLs, evidence paths, limitations, and clean-reproduction instructions. Capture only actual application screenshots. Do not score the result.


Also write `output/environment.json` with this execution contract. Paths are relative to `/workspace`; commands run there. Use your actual filenames and database credentials:

```json
{
  "compose_file": "registry/compose.yaml",
  "startup_command": "./registry/start.sh",
  "teardown_command": "./registry/stop.sh",
  "migration_command": "./registry/migrate.sh",
  "db_service": "db",
  "db_user": "fixture",
  "db_name": "registry_fixture"
}
```

Startup must build, migrate, seed, and wait for readiness. Migration must be safe to invoke again. Teardown must remove only the selected instance's containers, networks, and data volumes. All commands must honor `COMPOSE_PROJECT_NAME`, `WEB_PORT`, and `API_PORT` supplied by the executor. Do not use fixed container names, external shared volumes, or hard-coded host ports. The executor will start a second project with different ports, write test data, restart containers, and verify that cleanup preserves the other project.

Name the API Compose service `api` and the frontend service `web` and provide Node and Playwright Chromium there so the executor can inspect the real page. Keep runtime version and checksum evidence in your report.
