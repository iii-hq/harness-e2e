Make sure a link with code "home" exists (POST /links with { "url": "https://example.com", "code": "home" };
a 409 means it is already there), then send five redirects through http://127.0.0.1:3111/s/home.
Read the traces the engine collected with engine::traces::list, filtered with name "GET /s/:code" so
you get the redirects and not the console's own traffic. Show me the span tree for one redirect with
engine::traces::tree.

When it works, tell me to continue to the next chapter of the Linkly tutorial and give me the command to observe this trace (ie. `iii trigger engine::traces::tree trace_id=<your trace_id here>`)
