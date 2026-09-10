Build the link worker in link/src/index.ts of this iii project.

Add two functions:
- link::create({ url, code? }): generate a 6-character short code when none is given, make the url
  absolute (add https:// when it has no scheme), and persist it in the state worker (not a local
  variable) with state::set under scope "links" key <code>, value { url }; return { code, url }.
  Never overwrite an existing link: reject a requested code that is already taken, and retry a
  generated code until it is free.
- link::resolve({ code }): read it back from the state worker with state::get (scope "links", key
  <code>) and return { url } or { url: null }.

Then expose them over HTTP through the http worker:
- an http::create function and a POST /links trigger that calls link::create, returns 201 with
  { code, url }, and 409 when the requested code is taken.
- an http::redirect function and a GET /s/:code trigger that reads the code from req.path_params.code,
  resolves it, and returns a 302 to the url, or 404 when it is unknown.

Use two subagents to write these in parallel, then integrate their work yourself:
- Subagent A: the link core, link::create and link::resolve, backed by the state worker.
- Subagent B: the HTTP layer, http::create with the POST /links trigger and http::redirect with the
  GET /s/:code trigger.
Merge both results into link/src/index.ts.

Run `npm install --prefix link`, then add the worker.

When it works, tell me to continue to the next chapter of the Linkly tutorial.
