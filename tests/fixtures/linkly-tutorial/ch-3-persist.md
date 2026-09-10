Add a database, then configure it for SQLite, call it "primary". Keep state in front as a read cache for
recent requests and new links.

In link/src/index.ts:
- at startup, create a links table and a clicks table if they do not exist. The database worker may
  register a moment after link, so retry the schema creation until it answers, and never crash the
  worker if the database is not ready yet.
- link::create also inserts the link row into the database.
- link::resolve reads state first, falls back to the database, and warms the cache on a hit.
- add link::record_click that tracks clicks by link and day, and have http::redirect call it every time a link is requested.

When it works, tell me to continue to the next chapter of the Linkly tutorial.
