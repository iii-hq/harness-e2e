# Store one exact state value efficiently

## Plans

- daily
- weekly

## Version

2

## Before Test

The scope `e2emd-minimal-{{run_id}}` and key `minimal_result` are owned exclusively by this run. Use exactly one successful `state::set` to establish the baseline value `{"owner":"efficiency-suite","case_seed":{{seed}},"status":"prepared"}` at that scope and key, then use `state::get` to confirm it. Stop after confirming the prepared state. Make no other state changes.

## Prompt

Store exactly `{"owner":"efficiency-suite","case_seed":{{seed}},"status":"stored"}` in scope `e2emd-minimal-{{run_id}}` under key `minimal_result` using a single `state::set` call. Then reply with one short confirmation line containing `MIN-DONE`. Make no other function call except function discovery if it is necessary.

## Validations

### Exact result (40%)

Use `state::get` on scope `e2emd-minimal-{{run_id}}` and key `minimal_result`. Confirm that the final value exactly equals `{"owner":"efficiency-suite","case_seed":{{seed}},"status":"stored"}` and that trusted subject evidence contains exactly one successful `state::set` with those exact arguments.

### Single task call (13%)

Confirm from trusted subject evidence that the evaluated session made exactly one non-discovery function call and that it was the required `state::set`.

### At most two task calls (12%)

Confirm from trusted subject evidence that the evaluated session made no more than two non-discovery function calls. Both calls, if two occurred, must target only the owned scope and key; at least one must be the required successful `state::set`.

### Single turn (10%)

Confirm from trusted metrics that the evaluated session completed in exactly one turn.

### At most two turns (10%)

Confirm from trusted metrics that the evaluated session completed in no more than two turns.

### Zero friction (15%)

Confirm from trusted evidence that the evaluated session had zero function-call errors, made no non-discovery call besides the single required write, and returned a concise confirmation containing `MIN-DONE`.
