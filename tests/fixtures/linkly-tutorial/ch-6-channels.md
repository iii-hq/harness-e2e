Bulk-load links from a CSV in a single streamed upload over a channel.

- Create a bulk-importer worker that accepts a channel, reads CSV rows, and calls link::create for
  each. Skip a row the application rejects (a code already taken) instead of aborting the batch, but
  re-throw anything else (a timeout, a worker that is down). Return
  the imported and skipped counts.
- Create channel-client/import-links.js: a Node script that opens a channel and streams a small CSV
  of links to the importer.
- Create an example CSV for the user (example.csv). Do not import it. For the client's test run,
  create test.csv with exactly two rows whose codes are mylink and mydocslink (any https URLs).

Agree on the import_csv payload shape first, then run two subagents on separate files at the same
time:
- Subagent A: the bulk-importer worker (read the CSV off the channel, skip a rejected row but
  re-throw other failures, return the imported and skipped counts) and add it with compose.
- Subagent B: channel-client/import-links.js (open a channel, stream the CSV, call
  bulk-importer::import_csv).

When it works, tell me to continue to the next chapter of the Linkly tutorial.
