# Design system: derived from the Console

The Harness E2E extension renders inside the iii Console document. The Console
owns the visual language; the extension's design system (`src/design-system/`)
is a thin layer that consumes it. This document states what the extension
derives, from where, and what still stands on its own.

## Source of truth

The Console defines its tokens in one `@theme` block
(`console/web/src/index.css`) and its shared component recipes in
`console/web/src/styles/ui-recipes.css`. Both ship in the Console's own
stylesheet, so every custom property and every `.iii-ui-*` class below is
available to the extension at runtime without copying.

| Family | Host tokens | Notes |
|---|---|---|
| Fonts | `--font-sans` (Inter), `--font-mono` (Geist Mono), `--font-code` (Monaco) | Weights 400/500/600 shipped |
| Surface ramp | `--color-bg` → `--color-sidebar` → `--color-panel` → `--color-panel-raised` → `--color-surface` → `--color-surface-hover` → `--color-surface-selected` / `--color-surface-active`; `--color-card-highlight` | Hierarchy is carried by fills, not lines |
| Ink ramp | `--color-ink`, `--color-ink-faint`, `--color-muted-foreground`, `--color-ink-ghost`, `--color-ink-disabled` | |
| Borders | `--color-rule*` are transparent by design; `--color-edge` is the one structural stroke; `--color-rule-focus` is the focus ring | The system draws no 1px lines |
| Accent and status | `--color-accent` (+ `-fg`, `-hover`, `-muted`, `-border`), `--color-ok`, `--color-warn`, `--color-alert` (each with `-muted`) | Light: burnt orange; dark: blue accent |
| Glyph tones | `--color-glyph-{blue,purple,teal,green,amber,rose}` | Never a fill, border, selection or text |
| Radii | `--radius-{xs,sm,md,lg,xl}` = 6px, `--radius-none`, `--radius-full` | One radius everywhere |
| Elevation | `--shadow-raised`, `--shadow-floating`, `--shadow-lift`, `--shadow-keycap` | |
| Motion | `--motion-duration-{instant,fast,control,panel}`, `--motion-ease-{standard,enter,exit}`; `--duration-{stagger,micro,quick,fast,medium,slow,very-slow}`, `--ease-{smooth-out,in-out}` | Zeroed by the host under reduced motion |
| Type scale | `--text-{xs,sm,base,lg,xl,3xl}` with `--text-*--line-height`; `--leading-{tight,snug,relaxed}`; `--tracking-{tight,normal,wide}` | Tailwind defaults: 12 / 14 / 16 / 18 / 20 / 30 px |
| Spacing | `--spacing` (0.25rem base), `--spacing-gutter` 24px, `--spacing-section-x` 36px, `--spacing-content-max` 1216px | |

## What the extension derives today

Colors and fonts already resolved to host tokens through an alias layer
(`components/dashboard-shell.css`, `index.css` `@theme`). This branch makes
the rest of `design-system/foundations.css` derive as well.

| Extension token | Derives from | Fallback |
|---|---|---|
| `--ds-text-xs` | `--text-xs` | 0.75rem |
| `--ds-text-md` | `--text-sm` | 0.875rem |
| `--ds-text-lg` | `--text-base` | 1rem |
| `--ds-text-xl` | `--text-xl` | 1.25rem |
| `--ds-leading-tight` | `--leading-tight` | 1.25 |
| `--ds-leading-body` | `--text-base--line-height` | 1.5 |
| `--ds-tracking-tight` | `--tracking-tight` | -0.025em |
| `--ds-space-N` | `calc(var(--spacing) * N)` | 0.25rem × N |
| `--ds-radius-{sm,md,lg}` | `--radius-{sm,md,lg}` | 6px |
| `--ds-radius-pill` | `--radius-full` | 9999px |
| `--ds-duration-instant` | `--motion-duration-instant` | 0ms |
| `--ds-duration-fast` | `--motion-duration-fast` | 120ms |
| `--ds-duration-base` | `--motion-duration-panel` | 220ms |
| `--ds-duration-slow` | `--duration-very-slow` | 500ms |
| `--ds-ease-standard` | `--motion-ease-standard` | cubic-bezier(0.2, 0, 0, 1) |
| `--ds-ease-emphasized` | `--motion-ease-enter` | cubic-bezier(0.16, 1, 0.3, 1) |

Alias layer (already derived, kept until the last legacy page dies):

| Alias | Host token | Where |
|---|---|---|
| `--surface`, `--surface-raised`, `--surface-fill`, `--surface-soft`, `--surface-selected` | `--color-panel`, `--color-panel-raised`, `--color-surface`, `--color-surface-hover`, `--color-surface-selected` | `dashboard-shell.css` |
| `--text`, `--text-soft`, `--text-muted`, `--ink-decor` | `--color-ink`, `--color-ink-faint` (95% mix), same, `--color-ink-ghost` | `dashboard-shell.css` |
| `--line`, `--line-strong` | `--color-edge` | `dashboard-shell.css` |
| `--accent`, `--accent-soft` | `--color-accent`, `--color-accent-muted` | `dashboard-shell.css` |
| `--success`, `--warning`, `--danger` | `--color-ok`; `--color-warn` mixed 70% with ink; `--color-alert` mixed 80% with ink | contrast adjustments, see decisions |
| `--info` | `--color-accent` | the host has no info token |
| `text-ink-muted`, `text-ink-soft`, `bg-panel-*`, `text-warning`… (Tailwind) | the aliases above | `index.css` `@theme inline` |

Removed on this branch: the local `--radius-*` copies, `--font-body`
(undefined in the host, unused), the `--color-accent-fg` override, literal
`160ms ease` transitions in the shell, and the duplicate reduced-motion token
block (the host zeroes its motion tokens itself).

## Still extension-owned

These have no host equivalent. They are the open decisions of the redesign,
not settled facts.

| Token | Value | Why it exists | Options |
|---|---|---|---|
| `--ds-text-label` | 11px | mono uppercase labels over data | keep (the host itself uses 11px for accent text) |
| `--ds-text-sm` | 13px | body text in dense tables | merge into 14px (`--text-sm`) or 12px (`--text-xs`); the host's own table recipe uses 13px inside `@container (min-width: 40rem)` |
| `--ds-text-display` | 22px | page titles | use `--text-xl` (20px) |
| `--ds-tracking-label` | 0.06em | uppercase labels | keep |
| `--warning`, `--danger` mixes | ink-mixed | contrast on cream at 11–12px | drop the mixes and use the host colors at the sizes the host uses them |
| `--info` | accent | callouts | keep as accent, or drop the tone |

## Primitives against the host's recipes

The Console ships recipes the extension currently reimplements in
`primitives.css` (939 lines). The redesign adopts the recipe wherever one
exists; the primitive becomes the React wrapper that emits the recipe's markup.
The one exception is the section bar, which the extension owns (below).

| Extension primitive | Host recipe | Markup contract |
|---|---|---|
| `Panel` | `.iii-ui-panel` (static), `.iii-ui-card` (interactive: `data-interactive`, `data-selected`), `.iii-ui-card-highlight` (inset) | one element; state via data attributes |
| `FilterChip` / `FilterChipGroup` | `.iii-ui-chip[data-selected]` | inline-flex, 24px, `--color-surface` fill |
| `StatusBadge` | `.iii-ui-chip[data-tone=accent\|success\|warning\|danger]` | tone = host status colors with muted fills |
| `DataTable` / `DataTableRow` | `.iii-ui-table-viewport > .iii-ui-table-frame > table.iii-ui-table` with `__header`, `__body`, `__row[data-interactive\|data-selected]`, `__head`, `__cell`, `__caption`; `data-density="compact"` | rows separated by `--color-edge`, no vertical lines, container-query type size |
| section navigation (`.harness-e2e-nav-link`) | none: extension-owned, see below | links with `aria-current`, 2px ink underline, 44px targets |
| view toggles (sort, theme, grouped/by-test) | `.iii-ui-segmented > .iii-ui-segmented__item[aria-checked]` | |
| `Field` / `Input` / `Select` / `Textarea` | `.iii-ui-field` with `__label`, `__description`, `__error` | controls keep the shell's fill-only outline |
| toggles | `.iii-ui-switch` with `__input`, `__thumb` | |
| lists (plans, scenarios in a plan) | `.iii-ui-list > .iii-ui-list-group > .iii-ui-list-item[aria-current\|data-selected]` | |
| `Dialog` | host motion recipes `.iii-ui-motion-overlay`, `.iii-ui-motion-sheet` | the dialog itself stays a primitive |
| `Button` | none in the host (its buttons are utility compositions) | stays a primitive, tokenised |
| `EmptyState`, `Callout`, `MetricCard`, `DeltaValue`, `PageHeader` | none | stay primitives, tokenised |

The section bar does not adopt `.iii-ui-tabs-list > .iii-ui-tab`. That recipe
spaces tabs 20px apart with 2px of side padding, sets every tab in 600 at
40px and draws an inset 1px edge under the list; the redesign canvas wants a
44px raised strip with 10px of side padding, 4px gaps, 500 tabs with the
current one in 600, and only the 2px ink underline. The bar, its tabs and the
page actions beside them live in `components/dashboard-shell.css`, on host
tokens, with 44px targets.

Adopting a recipe is a per-primitive change with a visible effect on every
page, so it belongs to the redesign, screen by screen, not to this branch.

## Retirement plan for the alias layer

The three-name chain (`--color-ink-faint` → `--text-soft` → `text-ink-soft`)
exists only because legacy pages were written against it. New screens use host
names directly. When the last legacy page is gone, `dashboard-shell.css` loses
its alias block and `index.css` its `@theme inline`, and the CSS debt ratchet
(`tests/dashboard/css-debt.test.cjs`) goes to zero.
