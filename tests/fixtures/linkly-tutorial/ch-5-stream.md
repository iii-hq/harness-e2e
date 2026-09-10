Push every click to subscribers in real time.

- Create a click-streamer worker that stores every click in a "clicks" stream with stream::set
  (stream_name "clicks", group_id "all", one item per click, so stream::list reads them back), and add
  it with compose. Use the stream worker and its functionality.
- Have link::record_click publish each click so the streamer stores it. Build the stored item from the
  fields you need ({ code, clicked_at }) instead of forwarding the delivered payload as-is: the engine
  stamps bookkeeping fields such as _caller_worker_id on every delivery. We will make the subscribers
  in a later step.

Pick the topic name first, then run two subagents on separate files at the same time:
- Subagent A: the click-streamer worker that subscribes to that topic and stores each click in the
  "clicks" stream with stream::set, added with compose.
- Subagent B: the link::record_click change in link/src/index.ts that publishes each click on the
  topic.

When it works, tell me to continue to the next chapter of the Linkly tutorial.
