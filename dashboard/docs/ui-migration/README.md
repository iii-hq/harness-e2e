# UI migration briefs

The dashboard design audit of 2026-09-01 (316 findings, ids like `E-08` or
`AW-01`) is being closed in six pull requests. Wave 1 and the guard-rails are
delivered (#62, #71). What remains is one foundation PR and three page PRs:

| PR | Branch | Brief | Depends on |
|---|---|---|---|
| A | `ui/foundation` | [A-foundation.md](A-foundation.md) | #62, #71 |
| B | `ui/plans-and-setup` | [B-plans-and-setup.md](B-plans-and-setup.md) | A |
| C | `ui/tests-history-compare` | [C-tests-history-compare.md](C-tests-history-compare.md) | A · backend TH-07 |
| D | `ui/overview-executions-evidence` | [D-overview-executions-evidence.md](D-overview-executions-evidence.md) | A · backend E-12, CV-01 |

B, C and D are independent of each other and can run in parallel once A has
merged. The full finding text lives in [reports/](reports/); the design canvas
(mockups per page) is at
https://claude.ai/code/artifact/7ce1b04a-7e72-4432-82e2-9c0198379413 and the
roadmap at https://claude.ai/code/artifact/43057679-eab9-456b-aa6f-8393c64a92ab.

## Rules every UI PR follows

1. **Tests that pin copy change with the copy.** `tests/dashboard/overview-structure.test.cjs`
   and the component tests assert literal strings and class names on purpose.
   When a string changes, update the assertion to the new string. Never loosen
   an assertion into a regex or delete it to make CI pass.
2. **No `biome-ignore` without a reason in the comment**, and none for
   `useKeyWithClickEvents`, `noStaticElementInteractions` or
   `useSemanticElements` unless the element is genuinely a backdrop, a listbox
   child or a dialog. Fix the markup first.
3. **Only design-system vocabulary.** Buttons come from `buttonClassName` /
   `Button` (mono, lowercase, 6px radius, no border). Status is a dot plus
   text (`StatusBadge`). Labels are 11px mono (`text-[0.6875rem]`) and never
   smaller. Radii are `rounded-[6px]` or `rounded-full` for dots only. Borders
   only inside `.ds-*` or as table/input dividers.
4. **Colours are tokens.** New colours are `rgb(var(--he-*-rgb))` channel lists
   or `color-mix()` of host tokens. The console build rewrites unknown hex
   literals to `var(--color-ink)` and strips `box-shadow`, gradients and
   `@font-face`; anything you add that way silently disappears in the console.
5. **Tailwind classes are static strings.** Do not build class names by
   concatenating fragments (`text-[${size}]`); the compiler cannot see them.
   Put every variant as a literal in the file, even inside a helper.
6. **Cascade layers decide, not `important`.** Utilities sit above the
   `legacy` (legacy.css, dashboard-shell.css) and `ds` (design system) layers,
   so a utility on an element wins; unlayered host CSS in the console beats
   all of them. Never re-add `important` to the Tailwind import or to a rule.
   Toggle visibility from CSS keyed on `data-*` attributes, not from
   utilities, so the shell's rules stay the single source of truth.
7. **Dialogs use `showModal()`**, focus the title on open, have a 44×44 close
   control, and handle Escape on the element (Chromium groups a modal opened
   from an Escape press with the one below it, so `cancel` may fire on the
   parent). Backdrop click and Escape both mean "keep".
8. **Honest empty states.** "Not reported", "never run", "no assessments
   retained" are distinct; never show `0/0`, `undefined` or a placeholder that
   looks like data.
9. **Delete the legacy block you replace.** Each brief lists the `legacy.css`
   selectors that go with the page; the CSS-debt ratchet must go down, and
   `CSS_DEBT_UPDATE=1 node --test tests/dashboard/css-debt.test.cjs` locks the
   new floor in the same commit.
10. **One commit per surface**, message in the `fix:`/`feat:`/`refactor:` style
    with the audit ids in the body.

## Verification recipe

Run all of it before every commit:

```bash
pnpm --dir dashboard lint && pnpm --dir dashboard typecheck && pnpm --dir dashboard test
node --test tests/dashboard/*.test.cjs
pnpm --dir dashboard build          # standalone dist + dist-console
```

Then take live evidence against real data. The standalone dashboard serves the
same bundle the console embeds, so a Vite preview over the Rust server is
enough:

```bash
# terminal 1 — the Rust dashboard in local mode (needs iii on ws://127.0.0.1:49134;
# the harness functions are registered in the my-project namespace)
III_NAMESPACE=my-project target/debug/harness-e2e dashboard \
  --runs-dir ~/.iii/data/harness-e2e --listen 127.0.0.1:4173
# read-only variant without iii: add --view-only (no run suite, no catalog)

# terminal 2 — the built dashboard, proxying /api to 4173
pnpm --dir dashboard exec vite preview --port 4180 --strictPort

# terminal 3 — captures (every route × widths × themes) into dashboard/.screenshots/
pnpm --dir dashboard screenshots -- --base standalone
```

For ad-hoc probes use Playwright with `waitUntil: 'domcontentloaded'` and
fixed waits (`networkidle` never fires: the WebSocket keeps reconnecting).
Kill the servers with `pkill -f "[h]arness-e2e dashboard"` and
`pkill -f "[v]ite preview"` (the bracket keeps `pkill` from matching its own
shell). Two probe traps: the first `<select>` on any page is the shell's hidden
narrow navigation, so scope selectors to the page section; and the first
`<input>` in a dialog may be a hidden file input.

## PR body

- What changed per surface, with the audit ids closed.
- Ratchet numbers before and after (`css-debt.baseline.json` diff).
- The verification commands run, with test counts.
- A live-evidence table: what was checked, at which widths and themes, and
  the observed values. Say plainly what was not exercised live.
- End with the session link if the work was done with Claude Code.
