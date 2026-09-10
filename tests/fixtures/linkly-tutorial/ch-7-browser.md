Turn a browser tab into a worker.

- Add an rbac-proxy worker that fronts a public port 3110 and reverse-proxies to the engine at
  ws://127.0.0.1:49134, with an RBAC allowlist gated by auth::browser (expose link::create and
  link::request_delete only). Add it with compose.
- Create an auth worker with an auth::browser function that admits browser connections (fail closed
  when its LINKLY_BROWSER_TOKEN env var is unset). Each tab sends a unique session id; grant it its
  own `browser-<session>` namespace so two tabs never collide on the functions they register. Give it
  the standard node worker manifest (base_image `docker.io/iiidev/node:latest`, npm install and start
  scripts) and set `LINKLY_BROWSER_TOKEN: dev-token` in a .env.browser file at the root where
  worker-compose.yaml is. Add it with compose and set env_file on this worker to open .env.browser.
- Add link::delete and link::request_delete({ code, browser_namespace }): request_delete asks the
  tab's user::confirm_destructive_op in browser_namespace directly (the caller passes the full
  "browser-<uuid>" string; the handler uses it as-is with no prefix construction). before deleting.
- Create a Vite React app in frontend/ with `npm create vite@latest frontend -- --template
  react-ts`, install iii-browser-sdk, and write frontend/src/iii.ts and frontend/src/App.tsx that
  connect in a per-tab `browser-<session>` namespace, call link::create and subscribe to the click
  stream in the default namespace, and register user::confirm_destructive_op.
- In App.tsx, call link::request_delete with { code, browser_namespace: `browser-${SESSION_ID}` }.
  Never pass the raw UUID as a namespace argument; always pass the fully-constructed string so the
  server can use it without knowing the prefix convention.
- Have redirect links point at the http worker serving the redirects, not the frontend.
- Have the frontend show all existing generated links on load, and show the tab's own
  `browser-<session>` namespace on the page so it can be copied.

This chapter has the most independent pieces. Add and configure the rbac-proxy yourself first, since
the others depend on it, then run three subagents on separate directories at the same time:
- Subagent A: the auth worker in auth/ (the manifest and env described above, auth::browser, and the
  per-session `browser-<session>` namespace grant).
- Subagent B: the link worker changes in link/src/index.ts (link::delete and link::request_delete).
- Subagent C: the Vite app in frontend/ (iii.ts and App.tsx).

When it works, tell me the Linkly tutorial is complete.
