# Review the repository without changing it

Inspect the repository in `{{workspace}}` and write `review.json`. Do not modify
any existing file. The review must contain only evidence-backed findings with a
stable id, severity, path, line, explanation, and remediation.
Sort the findings by id so the report is deterministic.

Report the shell command injection and the unpinned third-party workflow action.
Do not report `.env.example` as a live-secret leak and do not invent package
vulnerabilities that cannot be proven from the repository.
