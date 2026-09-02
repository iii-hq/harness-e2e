# Implement bounded batch replay

Work in `{{workspace}}`. Implement `replay_all` so it consumes every event in
order using bounded pages, supports resuming strictly after an existing cursor,
does not duplicate events, and returns the number of handled events.

You may modify only `src/replay.py`. Preserve the store contract and tests, run
the complete public suite, and leave the implementation ready for hidden edge
cases including empty stores and exact page boundaries.
