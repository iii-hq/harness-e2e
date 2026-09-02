# Fix version-aware cache invalidation

Work in `{{workspace}}`. Reproduce the failing tests, then correct stale-event
handling across the service and store. Older events must not invalidate newer
entries, while a matching or newer event must invalidate the cached value.

The production correction must involve both `src/profile_service.py` and
`src/cache_store.py`. Do not modify tests. Run the full public suite before
finishing.
