# Reference plan — Registry version comparison

Status: draft v1 for human review. Input artifact for **Test 2 — Implementation using your plan**. This document defines work to perform; it does not report a completed implementation.

## 1. Identity and usage

- Application: `https://github.com/iii-hq/registry`.
- Required starting commit: `662eb87c1bdbb395f36264d5d26bf823e2ace783`.
- Source request: [Registry #17](https://github.com/iii-hq/registry/issues/17).
- Scenario: help users understand the differences between two versions before updating a worker.
- The executor must record the starting SHA, this plan's hash, the environment/fixture identity, and the final patch. Do not resolve `main` or `latest` to select the starting code.
- Environment fixture: the latest default-branch content of `iii-hq/e2e-fixture`, directory `registry-version-comparison/`, fetched at the start of each execution. Do not pin a fixture commit or reuse a stale checkout; keep the fetched copy unchanged during that execution. Its Dockerfile and Compose file pin base images by digest; iii 0.22.1 is verified by archive checksum. Built image identities are recorded with execution evidence.
- [Fixture setup and smoke check](https://github.com/iii-hq/e2e-fixture): API, PostgreSQL/pgvector, Next.js, public seed, local artifacts, and baseline browser screenshots are implemented. Record checksums of the copied fixture files as evidence; no fixture commit is required.
- Provide this plan only to Test 2. Test 1 receives requirements without this plan or documents that reveal the solution. Test 3 prepares the environment; Test 4 tests the patch from Test 2 without modifying it.
- At this stage, present artifacts, command results, and screenshots; do not calculate scores, rankings, or automated visual assessments.

## 2. Product deliverable

Add a **Changelog** tab to `/workers/:slug`. Users must be able to view release history, select source and target versions, inspect grouped differences, expand previous/new values, reverse the comparison, and share its URL.

Compare functions, triggers, dependencies, configuration, and artifacts. Highlight potentially breaking changes according to the limited policy in this document; do not claim that the analysis proves full behavioral compatibility.

Scope adapted from issue #17:

- Show history with versions and publication dates, reusing existing data. Do not create a second timeline API when `/w/:slug/versions` already supports the history.
- Return structured comparisons through the API and consume them in the interface. Do not implement a second comparison algorithm in the frontend.
- Exclude binary-size and 30-day download charts: the schema does not store artifact sizes, and available per-version telemetry is weekly. Do not invent metrics or change ingestion to obtain them in this scenario.
- Exclude changes to the CLI, publishing, channels, dependency resolution, source history, or external integrations.
- The original issue's cache-hit indicator is not required for this scope. Prioritize correct results and do not introduce a cache service.
- Do not require a JSON Patch library or a database migration: the necessary fields already exist.

## 3. Integration points verified at the starting commit

All paths below are relative to the Registry repository, not Harness E2E.

| Path | Reuse and considerations |
| --- | --- |
| `api/src/db/schema.ts` | `workerVersion` contains `functions`, `triggers`, `dependencies`, `config`, `binaries`, `imageTag`, and `createdAt`. |
| `api/src/lib/types.ts` | Function contracts: `request_schema`/`response_schema`; trigger contracts: `invocation_schema`/`return_schema`. |
| `api/src/repositories/worker.repository.ts` | `findByName` identifies the worker; `findVersionBySemver` reads an exact version by `workerId`. `findVersionsByWorkerName` supplies the existing history. |
| `api/src/services/worker.service.ts` | `listVersions` and existing readers enforce hidden-worker rules. Readers normalize some fields; comparison must preserve `null` when it indicates missing metadata. |
| `api/src/controllers/workers.versions.list.ts` | Existing HTTP registration pattern using `fn(...).http(...)` and `ApiResponse`. |
| `api/src/main.ts` | Import the new controller so its function/route is registered. |
| `app/src/app/workers/[slug]/page.tsx` | Tab integration and parameter parsing. Existing selection can fall back to the latest version: do not reuse this behavior for the comparison's source/target. |
| `app/src/components/page-tabs.tsx` | Add `changelog` to the existing type and navigation. |
| `app/src/components/versions-panel.tsx` | Reference for appearance, versions, and links; preserve the Versions tab. |
| `app/src/lib/data.ts`, `app/src/lib/types.ts` | Add response consumption and its contract. Do not turn a comparison failure into an empty list. |
| `api/vitest.config.ts`, `api/tests/setup/` | Existing API suite with database and global setup. |
| `app/playwright.config.ts`, `app/e2e/` | Browser tests share the API database; already uses one worker and `E2E_BYPASS_CACHE`. |

Immutable sources: [schema](https://github.com/iii-hq/registry/blob/662eb87c1bdbb395f36264d5d26bf823e2ace783/api/src/db/schema.ts), [repository](https://github.com/iii-hq/registry/blob/662eb87c1bdbb395f36264d5d26bf823e2ace783/api/src/repositories/worker.repository.ts), [service](https://github.com/iii-hq/registry/blob/662eb87c1bdbb395f36264d5d26bf823e2ace783/api/src/services/worker.service.ts), [page](https://github.com/iii-hq/registry/blob/662eb87c1bdbb395f36264d5d26bf823e2ace783/app/src/app/workers/%5Bslug%5D/page.tsx), [Playwright](https://github.com/iii-hq/registry/blob/662eb87c1bdbb395f36264d5d26bf823e2ace783/app/playwright.config.ts).

## 4. Comparison contract

Add `GET /w/:slug/compare/:from...:to`.

- `from` and `to` are existing exact SemVer versions, including prereleases. Do not accept tags such as `latest`, ranges, or automatic substitution with another version.
- Invalid SemVer: HTTP 400, `error.code = invalid_version`.
- Missing or hidden worker: HTTP 404, `error.code = worker_not_found`.
- Version missing within that worker: HTTP 404, `error.code = version_not_found`, with `error.version` identifying the first missing version (source before target).
- Deprecated workers remain queryable, preserving the current detail policy.
- An unexpected database/API error must remain an error, never HTTP 200 with an empty comparison.
- Results are directional: reversing source/target swaps `added`/`removed` and `before`/`after`, and recalculates impact.

Success format, illustrating a comparison with no changes:

```json
{
  "worker": "orders-worker",
  "from": "1.0.0",
  "to": "1.0.0",
  "changes": [],
  "unavailable": []
}
```

Each `changes` entry contains:

- `area`: `functions`, `triggers`, `dependencies`, `config`, or `artifacts`.
- `name`: function, trigger, dependency, or target name; omit for configuration.
- `path`: JSON Pointer within the compared entity; `""` represents the entire entity.
- `kind`: `added`, `removed`, or `changed`.
- `impact`: `potentially_breaking`, `additive`, or `review`.
- `before`: previous value, present for `removed`/`changed`.
- `after`: new value, present for `added`/`changed`.

Omit the nonexistent side; do not use `null` as a missing-property sentinel, since `null` can be an actual configuration value. Sort by area in the order above, then by name, path, and kind for deterministic output. The UI may show warnings first without changing the API contract.

Each `unavailable` entry contains `area`, `side` (`from` or `to`), and `reason` (`metadata_missing`). If an area is unavailable on either side, do not fabricate differences for that area.

### Equality, missing data, and limits

- Ignore object key order at every depth.
- Identify functions, triggers, and dependencies by `name`; identify binaries by target rather than position.
- In schemas, treat `required` and `enum` as sets; preserve order in other arrays. In configuration, order remains significant, including arrays named `required` or `enum`.
- A persisted empty function/trigger array represents the stored information: the schema has no marker distinguishing metadata that was never published from an intentionally empty set. Do not infer history the database does not record.
- `config: null` means unavailable metadata; `{}` is known, empty configuration. For binary workers, `binaries: null` is unavailable; a known map missing a target allows its removal to be detected.
- For `image`, compare `imageTag`; for `bundle`, compare the `*` artifact as a bundle archive without presenting it as an operating-system platform. Missing values for these types are also unavailable.
- Comparing a version with itself produces `changes: []`; areas without metadata still appear in `unavailable`.
- Do not attempt to prove general JSON Schema equivalence or resolve `$ref` over the network. Structural changes outside the rules below are `review`.

### Impact policy v1

| Change | Impact |
| --- | --- |
| Function or trigger removed | `potentially_breaking` |
| Function or trigger added | `additive` |
| Field added to the input's root `required` (`request_schema`/`invocation_schema`) | `potentially_breaking` |
| Field removed from the input's root `required` | `additive` |
| Optional property added at the input root, with all other constraints unchanged | `additive` |
| Binary target removed / added | `potentially_breaking` / `additive` |
| Other schema changes, including output, types, enums, nested structures, and `$ref` | `review` |
| Dependency, configuration, description/metadata, hash, artifact URL, or image changed | `review` |

Emit separate entries for independent changes; an optional addition must not hide a simultaneous type change. For root `required`, emit one set change with `before`/`after`; if additions and removals occur together, use `potentially_breaking`. When an entire entity is added/removed, do not duplicate changes for all its children. The UI uses explanatory text without concluding that an update is safe merely because no warning is present.

When a new property also enters root `required`, emit **two entries**: the property addition and the `required` set change, both `potentially_breaking`. Count entries, not distinct fields or functions. Sort serialized `required` sets by name. For the fixture's `currency` case, the entries are exactly:

```json
[
  {
    "area": "functions",
    "name": "orders::create",
    "path": "/request_schema/properties/currency",
    "kind": "added",
    "impact": "potentially_breaking",
    "after": { "type": "string" }
  },
  {
    "area": "functions",
    "name": "orders::create",
    "path": "/request_schema/required",
    "kind": "changed",
    "impact": "potentially_breaking",
    "before": ["customerId"],
    "after": ["currency", "customerId"]
  }
]
```

When reversing this case, property removal receives `review` (there is no general compatibility rule for removing properties), and the reduction in `required` receives `additive`, with before/after reversed.

## 5. Public fixture data

Use `orders-worker`, of type `binary`, with fixed UTC dates. Do not depend on actual published versions or fetch remote artifacts. The specifications below are public inputs and may be known to the implementer; reserved variants stay outside the workspace.

| Version | Data |
| --- | --- |
| `0.9.0` — 2026-01-01 | `config: null`, `binaries: null`, empty functions/triggers. Used to display unavailable metadata. |
| `1.0.0` — 2026-02-01 | `orders::create`, `orders::get`; create input is an object with required `customerId: string`. Response `{id: string}`, with required `id`. Config `{timeoutMs: 3000}`. Dependency `storage-worker: ^1.0.0`. Targets `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`. Trigger `orders.created`, with fixed schemas. |
| `1.1.0` — 2026-03-01 | Inherits 1.0.0; adds `orders::list` and optional `reference: string` to create input. Preserves other contracts and artifacts. |
| `2.0.0` — 2026-04-01 | Inherits 1.1.0; removes get; adds required `currency: string` to create input; changes timeout to 5000 and storage to `^2.0.0`; removes the macOS target, changes the Linux SHA; removes `orders.created` and adds `orders.accepted`. |

The fixture supplies complete schemas, timestamps, SHA-256 hashes, and local artifact paths in `seed.sql` and `artifacts/`. The seed also includes `route/name` and `worker~mode` configuration keys to exercise escaped paths; `route/name` changes in 2.0.0. These public fixture files are authoritative for exact values. Artifact origins use the configured web port, which must remain consistent for a reproduction. The smoke check verifies local HTTP access and matching hashes. The artifacts are text payloads for download verification, not executable worker binaries.

The fixture includes `no-version-worker` and `second-worker@9.9.9` to verify lookup scope, and permuted object keys/required arrays in its sample schemas. PostgreSQL JSONB normalizes object key order; test additional pure comparison inputs for this behavior. Seeding uses controlled insertion after existing Registry migrations, without changes to publishing.

## 6. Test 2 implementation sequence

### Step A — Inspect and establish the baseline

1. Confirm the starting SHA, read repository instructions, and locate the integration points in section 3.
2. Use the executor-provided environment; confirm the API, database, and frontend are reachable. The current CI Compose includes Postgres/pgvector and the API, but does not by itself constitute the complete scenario environment.
3. Run the public baseline checks specified by the fixture and record preexisting failures before modifying code. If the provided environment does not start, report an infrastructure blocker.

### Step B — Build backend comparison

1. Read both exact versions of the same worker, respecting visibility. Reuse existing readers; do not use the normalized UI projection as the source for missing metadata.
2. Create a comparison implementation without network/database access, called by the service. Add only the necessary modules following the current structure; do not create a diff framework or generic provider abstraction.
3. Implement identity by name/target, structural equality, paths, missing values, and impact policy. Test directional behavior before building the UI.
4. Expose the endpoint and register the controller in `api/src/main.ts`. Follow the existing response type pattern and reflect the route in the project's API documentation/schema.

### Step C — Integrate the Registry experience

1. Add the Changelog tab to the existing page. Reuse `/w/:slug/versions` to show releases by descending date; break ties by sorting exact version strings for stable ordering.
2. Use `/workers/:slug?tab=changelog&from=1.0.0&to=2.0.0` as shareable state. `from`/`to` are independent of the existing tabs' `version` parameter.
3. With no pair selected, show history and selectors inviting the user to compare. Do not silently choose `latest`. With only one side selected, request the other; with an invalid value, show an explicit error.
4. Consume the API using the frontend's existing access pattern. Render groups, counts derived from changes, unavailable-metadata notices, and expandable before/after values.
5. Implement reversal and link copying. Browser back/forward navigation must restore the pair. A loading failure must not leave a previous pair's differences under new labels.
6. Use existing components/styles, labeled controls, keyboard operation, and accessible expansion. Do not rely on color alone to indicate impact. Do not add new analytics events in this scenario.

### Step D — Run tests and deliver

1. Cover the algorithm and HTTP contract with Vitest, including database lookups by worker/version and the errors in section 4.
2. Add Playwright flows using the real frontend/API/database. Preserve serial execution within each environment; scenarios have independent environments for parallel execution.
3. Run checks, start the application, and navigate through the screens. Record actual results, including failures; do not change expectations merely to obtain a green run.
4. Deliver the patch and a short report: changes, commands, results, deviations from this plan, and limitations. Do not publish, promote versions, or call production services.

## 7. Minimum delivery checks

| Case | Expected observation |
| --- | --- |
| `1.0.0 → 1.1.0` | list and reference added; no removals or potentially breaking warnings. |
| `1.0.0 → 2.0.0` | get removed, currency required, macOS removed, trigger replaced; dependency/config/hash changes with correct values. |
| Reversed comparison | Before/after and additions/removals reversed; impact recalculated. |
| Version compared with itself | No differences; preserve missing-metadata notices, if any. |
| Object / required / enum order | No differences caused solely by permutation; configuration arrays remain ordered. |
| Missing property versus null | Do not confuse absence with a JSON null value. |
| Missing or hidden worker/version | Error according to the contract; do not substitute the latest version or another worker's version. |
| 0.9.0 data | Unavailability notices; do not present missing maps as platform/configuration removals. |
| Shared URL and browser history | Same pair and result after reopening or navigating. |
| API error after changing the pair | Explicit error; do not display the previous result as current. |
| Regression | Versions, README, version selection, API reference, and downloads retain existing behavior. |

Existing commands to run in the Registry checkout after the provided environment/seed is ready:

```bash
pnpm --dir api typecheck
pnpm --dir api test
pnpm --dir api build
pnpm --dir app typecheck
pnpm --dir app test
pnpm --dir app lint
pnpm --dir app build
pnpm --dir app e2e
git diff --check
```

The API suite has global database setup; E2E tests need the API running, a local `DATABASE_URL`, and consistent `TEST_API_URL`, `E2E_APP_PORT`, and, when necessary, `E2E_APP_URL`. Do not treat these commands as independent of the fixture. Install dependencies from the lockfile using pnpm 10.19.0 and Node >=22, as specified by the starting commit's `package.json`; pin the actual versions in the fixture image.

## 8. Evidence and handoff to Test 4

The executor applies the patch to a clean copy of the same SHA, uses the provided environment, and loads the same data. Do not reuse undelivered files, processes, or the database from the AI session.

Capture through the browser, without automated visual assessment:

| Proposed file | Content |
| --- | --- |
| `01-changelog.png` | Release history. |
| `02-additive.png` | Comparison 1.0.0 → 1.1.0. |
| `03-breaking.png` | Comparison 1.0.0 → 2.0.0 with warnings. |
| `04-detail.png` | Expanded timeout before/after values. |

Use a fixed 1440 × 1000 viewport, wait for content to load, and record the URL, caption, and code/patch/fixture identities with each capture. If the application does not start or a control is missing, record the capture as unavailable with its reason and logs; do not fabricate images or substitute a mockup. Executor-controlled capture is separate from tests and screenshots produced by the AI during development.

Test 4 receives the resulting implementation, requirements, and access to the isolated instance with a real API/database. It tests the flows and produces an expected/observed report with evidence, without fixing the code. It does not receive a test-generation suite as a separate scenario. The environment built in Test 3 can be reproduced separately; its failure must not replace the prepared environment used to test the feature.

## 9. Artifact readiness boundary

This plan remains a draft for review. The fixture provides reproducible startup, public seed data, local artifact verification, and baseline browser capture. Clean database replay and 25 existing API integration tests passed during fixture preparation. The comparison feature has not been implemented. Remaining work: a public task statement aligned with the contracts above (without implementation instructions), executable scenarios, feature-specific capture/gallery integration, and patch transfer to Test 4. Baseline screenshots do not demonstrate a completed Changelog feature.
