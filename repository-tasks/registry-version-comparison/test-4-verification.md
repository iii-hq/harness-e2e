# Test 4 — Verify the delivered feature in a real environment

The `registry/` workspace contains the Test 2 delivery applied to the pinned starting commit. Read `inputs/requirements.md` and `inputs/environment.json`. Treat the running API, frontend, and PostgreSQL instance as the system under test.

Exercise the required flows through the real API and browser: additive, breaking, reverse, same-version, unavailable metadata, invalid/missing versions, worker scoping, shareable URL and browser history, error/stale-state behavior, expandable values, accessibility basics, local downloads, and preserved existing Registry flows. Record expected and observed behavior with reproducible steps, URLs, responses or logs, and actual 1440 × 1000 screenshots. Do not generate mock images.

Do not edit product source or fix defects. Do not change requirements to make the delivery pass. Write `output/report.md` in English, classify each check as observed pass, observed failure, or blocked with evidence, and place screenshots and raw evidence under `output/`. Do not score or rank the implementation.
