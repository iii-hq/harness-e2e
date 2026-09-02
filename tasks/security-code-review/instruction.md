# Review the repository without changing it

Inspect the repository in `{{workspace}}` and write `review.json`. Do not modify
any existing file. The review must contain only evidence-backed findings with a
stable id, severity, path, line, explanation, and remediation.

Use exactly this top-level shape, with no additional fields:

```json
{"summary":"...","findings":[{"id":"...","severity":"...","path":"...","line":1,"explanation":"...","remediation":"..."}]}
```

Use these stable ids for the corresponding issue classes and sort by id:

- `shell-command-injection` for shell command injection;
- `unverified-remote-script` for downloading and executing an unverified script;
- `unpinned-workflow-action` for an action referenced by a mutable ref.

Inspect source code, workflows, dependency metadata, and configuration examples.
Report every concrete security issue that can be proven from repository content,
but exclude examples and dependency claims that lack evidence in the checkout.
