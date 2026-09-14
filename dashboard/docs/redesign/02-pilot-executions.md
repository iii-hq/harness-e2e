# Pilot: the executions ledger on the Console's components

The first screen rebuilt under the redesign. It sets the pattern every other
screen follows: the Console's own components and token names, logic outside
the page, no page-specific CSS.

## What the Console already gives us

The host injects a React component library into every extension as
`@iii-dev/console-ui` (`console/web/src/lib/console-api.ts`): `Badge`,
`Button`, `Card`/`Panel`, `Chip`, `Dialog`, `DropdownMenu`, `EmptyState`,
`Input`, `List`, `Select`, `SegmentedControl`, `Skeleton`, `StatusDot`,
`StatusPanel`, `Switch`, the `Table` family (`TableViewport`, `TableFrame`,
`Table`, `TableHeader`, `TableBody`, `TableRow`, `TableHead`, `TableCell`,
`TableCaption`), `Tabs`, `Tooltip` and more. They render the recipes in
`ui-recipes.css` and the Tailwind tokens of the host, so an extension that
uses them looks like the Console by construction.

The extension's design system therefore shrinks to what the host lacks. On
this screen that is only `PageHeader` (title, context line, one-sentence
summary). `Button`, `StatusBadge`, `DataTable`, `FilterChip`, `EmptyState`,
`Callout`, `Input` and `Select` from `src/design-system/` are not used here
and retire once the last legacy page stops using them.

## The screen

Route `#/ext/harness-e2e/executions` (the default). Job: find a recent
execution and open it.

| Part | Built with | Change from the previous page |
|---|---|---|
| Actions in the section bar | host `Button` (`primary` run tests, `pill` new plan) | same actions, host styling instead of `dashboardHeaderActionClassName` |
| Header | `PageHeader` | one sentence with one denominator: `N executions retained · M running · L loaded`, the last two only when they add information |
| Toolbar | host `Input` (search), `Button variant="icon"` (clear), host `Select` (trigger when more than one, sort), result filters as host `Button variant="pill"` with `aria-pressed` | one wrapping row; the `showing X of Y` counter appears only while a filter is active |
| Ledger | host `Table` (`density="compact"`) inside `TableViewport`/`TableFrame`; one `TableBody` per group; group heading as a `colgroup` `TableHead`; rows `interactive` | the `open` column is gone (the row and the title link open the execution); seven columns, `scope` renamed `tests` (reports received / expected, spelled out in the cell's title); the meta line under the title no longer repeats `local · local` for the default case; horizontal scroll in narrow containers instead of card collapse, as the host's own tables do |
| Result | host `Badge`: `ok` passed, `alert` failed, `warn` inconclusive / incomplete / cancelling, `accent` running, `default` cancelled / unavailable | one status vocabulary, the host's |
| Loading | host `Skeleton` × 6 | |
| Error | host `StatusPanel variant="alert"` with a retry `Button` | |
| Empty | host `EmptyState` with `run tests` (nothing retained) or `clear filters` (filtered out) | the primary action is inside the empty state, not only in the bar |

Logic (filters, sort, day and plan grouping, group statistics) moved
unchanged to `src/lib/executions-ledger.ts` with its tests; the page file
only renders.

## A layering trap

Both stylesheets declare Tailwind's layer names. The host's sheet loads first
and fixes the order `theme, base, components, utilities`; the extension's
`legacy` and `ds` layers are then appended after `utilities`, so any element
rule in them outranks the host's utilities on the host's own components.
`legacy.css` used to reset `button, select { color: var(--text) }`, which
painted the host's primary button ink on ink. The reset is gone; the rule
for the redesign is that nothing in `legacy` or `ds` may target bare
elements the host components render.

## Tokens

The page uses the host's names through Tailwind utilities: `text-ink`,
`text-ink-faint`, `bg-panel`, `bg-surface-selected`. `index.css` lists the
host colours under `@theme reference`, which makes the utilities exist
without redeclaring any value. The alias layer (`--text-muted`,
`text-ink-muted`, `bg-panel-subtle`…) is not used on this screen.

## Testing

Unit tests render `LedgerTable` through the test stub of
`@iii-dev/console-ui` (`test/console-ui.tsx`), which mirrors the host
markup (`iii-ui-table`, `data-interactive`, `data-badge-variant`) so the
assertions read the same attributes the Console renders. Visual checks run
the built bundle over the live Console with route interception
(`scripts` in the PR), light and dark, wide and 640px.

## What the next screens inherit

1. Import from `@iii-dev/console-ui` first; reach for `src/design-system/`
   only for what the host has no component for.
2. Host token names only; no alias tokens, no page CSS.
3. Logic in `src/lib/<screen>.ts` with tests; the page renders.
4. States designed, not defaulted: loading, error with retry, empty with the
   primary action, filtered-empty with a way back.
5. One primary action per screen, present where the person is when they need
   it.
