# Registry version comparison — public requirements

Starting repository: `iii-hq/registry` at commit `662eb87c1bdbb395f36264d5d26bf823e2ace783`.

Build a Changelog tab on `/workers/:slug` that lets a user review release history, choose exact source and target versions, inspect grouped differences, expand before/after values, reverse the comparison, and share the selected comparison by URL. The API must compute the comparison; the frontend must display that result rather than implement a separate comparison.

Show release history using stored versions and publication dates, newest date first; break date ties by exact version string for stable ordering.

Use `/workers/:slug?tab=changelog&from=1.0.0&to=2.0.0` for shareable state. With no pair selected, show history and selectors. Never silently choose a version. A partial or invalid selection must show an explicit prompt or error. Browser back/forward and reopening the URL must restore the pair. Failed loading must not leave stale differences under new labels. Controls and expandable details must be keyboard accessible, and impact must not rely on color alone.

## API contract

Add `GET /w/:slug/compare/:from...:to`.

- Accept existing exact SemVer values, including prereleases. Reject tags, ranges, and invalid SemVer.
- Invalid SemVer: HTTP 400 with `error.code = "invalid_version"`.
- Missing or hidden worker: HTTP 404 with `error.code = "worker_not_found"`.
- Missing version: HTTP 404 with `error.code = "version_not_found"` and `error.version` naming the first missing version, checking source before target.
- Deprecated workers remain queryable.
- Unexpected failures remain errors; never return an empty successful comparison.
- Reversing versions must swap additions/removals and before/after values and recalculate impact.

Success response:

```json
{
  "worker": "orders-worker",
  "from": "1.0.0",
  "to": "1.0.0",
  "changes": [],
  "unavailable": []
}
```

Each change has:

- `area`: `functions`, `triggers`, `dependencies`, `config`, or `artifacts`;
- `name`: entity or artifact target name, omitted for configuration;
- `path`: JSON Pointer inside the entity, with `""` for the whole entity;
- `kind`: `added`, `removed`, or `changed`;
- `impact`: `potentially_breaking`, `additive`, or `review`;
- `before`: present for removed and changed values;
- `after`: present for added and changed values.

Omit the nonexistent side; JSON `null` is a real value, not a missing-property marker. Sort by area in the order above, then name, path, and kind. Each unavailable entry has `area`, `side` (`from` or `to`), and `reason: "metadata_missing"`. If either side lacks an area's metadata, report it as unavailable and do not fabricate changes for that area.

## Comparison rules

- Ignore object-key order at every depth.
- Identify functions, triggers, and dependencies by `name`; identify binary artifacts by target.
- Treat schema `required` and `enum` arrays as sets. Preserve all other array order. Configuration arrays remain ordered even when named `required` or `enum`.
- Empty function/trigger arrays are known stored values. `config: null` is unavailable while `{}` is known empty configuration. For binary workers, `binaries: null` is unavailable; a known map missing a target represents removal.
- Compare `imageTag` for image workers and the `*` archive for bundle workers. Do not present a bundle as an operating-system target. Missing image tags or bundle archives are unavailable metadata.
- Comparing a version with itself returns no changes while preserving unavailable notices.
- Do not resolve external `$ref` values or claim general JSON Schema equivalence.

Impact policy:

| Change | Impact |
| --- | --- |
| Function or trigger removed / added | `potentially_breaking` / `additive` |
| Field added to input root `required` | `potentially_breaking` |
| Field removed from input root `required` | `additive` |
| Optional input-root property added with other constraints unchanged | `additive` |
| Binary target removed / added | `potentially_breaking` / `additive` |
| Other schema change | `review` |
| Dependency, configuration, metadata, hash, URL, or image change | `review` |

Emit separate entries for independent changes. Do not emit child changes when an entire entity is added or removed. A property newly added and also added to root `required` produces both the property addition and required-set change as `potentially_breaking`. For root `required`, emit one set change with before/after; simultaneous additions and removals are `potentially_breaking`. Sort serialized required sets. In reverse, removing that property is `review` and reducing the required set is `additive`.

## Public fixture data

The environment contains `orders-worker` versions with fixed dates:

- `0.9.0`: null configuration and binaries, empty functions and triggers.
- `1.0.0`: `orders::create` and `orders::get`; create requires string `customerId`; response requires string `id`; `{timeoutMs: 3000}` plus configuration keys containing `/` and `~`; dependency `storage-worker@^1.0.0`; Linux and macOS artifacts; trigger `orders.created`.
- `1.1.0`: adds `orders::list` and optional string `reference` to create; preserves the other contracts and artifacts. Equivalent objects and schema sets may use different source order.
- `2.0.0`: removes get; adds required string `currency` to create; changes timeout to 5000, a slash-containing configuration value, storage dependency to `^2.0.0`, and Linux hash; removes macOS; replaces `orders.created` with `orders.accepted`.

It also contains storage dependency versions, a worker with no versions, and `second-worker@9.9.9`, which must not satisfy an `orders-worker` lookup. Local artifact contents match their SHA-256 values and remain downloadable.

## Delivery and evidence

Preserve existing Versions, README, version selection, API reference, and download behavior. Include focused automated tests. Run relevant existing typechecks, tests, builds, lint, and browser tests available in the supplied environment and report exact commands and outcomes.

For implementation results, start the real application and capture actual browser screenshots at 1440 × 1000 for release history, `1.0.0 → 1.1.0`, `1.0.0 → 2.0.0`, and an expanded detail. Record unavailable captures and their cause; do not generate or mock screenshots. Do not score, rank, publish, promote, call production services, change CLI/publishing behavior, add binary/download charts, or add automated visual assessment.
