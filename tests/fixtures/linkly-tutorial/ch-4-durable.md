Make redirects fast and add analytics.

- Define a standard "clicks" queue with queue::define and enqueue click records instead of writing them inline on the redirect path.
- Add the pubsub worker. link::create publishes a link.created event.
- Add a second SQLite database named "analytics" for the analytics worker.
- Create a Python analytics worker that subscribes to link.created and counts links per day in a
  daily_link_counts table (day TEXT PRIMARY KEY, count INTEGER NOT NULL) in the analytics database,
  and add it with compose.
- Add link::update and a PUT /links/:code route, publish link.updated durably, and refresh the cache
  from a durable subscriber.

Add the pubsub worker and the second database yourself first, then split the rest across two
subagents that work on separate files at the same time:
- Subagent A: the Python analytics worker (subscribe to link.created, count links per day, write the
  daily_link_counts rows to the analytics database) and add it with compose.
- Subagent B: the link worker changes in link/src/index.ts (define and enqueue the clicks queue,
  publish link.created, add link::update with the PUT /links/:code route, publish link.updated
  durably, and refresh the cache from a durable subscriber).

When it works, tell me to continue to the next chapter of the Linkly tutorial.
