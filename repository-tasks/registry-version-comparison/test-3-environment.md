# Test 3 — Prepare a reproducible Registry environment

Work in `registry/`, pinned to the required commit. Read `inputs/requirements.md`, the public seed and artifacts under `inputs/`, and `inputs/environment.json`. Build the development environment needed to install, migrate, seed, start, and verify PostgreSQL/pgvector, the Registry API, and the web application.

Create the required Dockerfile, Compose configuration, and minimal scripts yourself. No prepared Dockerfile, Compose file, or smoke script is supplied. Pin runtime versions, keep each attempt isolated, use the supplied deterministic seed and local artifacts, and document one-command startup, verification, logs, and teardown. Verify the pinned Registry SHA, database migrations, seeded versions, API health, local artifact downloads and hashes, and real browser access to the existing Registry UI. Do not implement the comparison feature or edit its product behavior.

Leave the environment implementation in `registry/`. Write `output/report.md` in English with architecture, exact commands and outcomes, image/runtime identities, URLs, evidence paths, limitations, and clean-reproduction instructions. Capture only actual application screenshots. Do not score the result.
