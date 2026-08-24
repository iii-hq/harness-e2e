# Release policy changelog — 2025-02

Status: current
Authority: policy-change

The automatic retry rule for authorization and policy denials was removed. A denial now closes the
attempt. Remediation must produce new evidence and a new release attempt with a fresh idempotency
key. Transient transport failures may still be retried when no mutation receipt exists.

This entry supersedes the retry guidance in FAQ 2023.
