import { ChevronDown } from 'lucide-react'
import type { ReactNode, SyntheticEvent } from 'react'

/** One layer of a page that shows its metrics first and everything else on
 *  demand. The closed row carries a scent — enough to decide whether to open
 *  it — so hiding never means losing (audit ED-26). The row is the shape the
 *  dashboard already uses for "attempts" and "Inspect scenario
 *  evidence": chevron, bold label, mono summary, a fill rather than a border. */
export function DisclosureLayer({
  id,
  label,
  scent,
  open,
  onToggle,
  actions,
  children,
}: {
  id: string
  label: string
  /** Shown while closed; hidden once the body is visible. */
  scent?: string | null
  open: boolean
  onToggle?: (open: boolean) => void
  /** Shown while open, where the scent was. */
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <details
      id={id}
      className="group min-w-0 scroll-mt-24 rounded-[6px] bg-[var(--surface-fill)]"
      open={open}
      onToggle={(event: SyntheticEvent<HTMLDetailsElement>) =>
        onToggle?.(event.currentTarget.open)
      }
      data-layer={id}
    >
      <summary className="flex min-h-12 cursor-pointer list-none items-center gap-3 px-4 py-3 text-sm font-semibold text-ink marker:hidden">
        <ChevronDown
          className="size-4 shrink-0 -rotate-90 text-ink-muted transition-transform duration-[var(--ds-duration-fast)] group-open:rotate-0 motion-reduce:transition-none"
          aria-hidden="true"
        />
        <span className="whitespace-nowrap">{label}</span>
        {scent ? (
          <span
            className="ml-auto min-w-0 truncate font-mono text-label font-normal text-ink-muted group-open:hidden"
            data-layer-scent
          >
            {scent}
          </span>
        ) : null}
        {actions ? (
          <span className="ml-auto hidden shrink-0 items-center gap-2 group-open:flex">
            {actions}
          </span>
        ) : null}
      </summary>
      <div className="min-w-0 px-4 pb-4">{children}</div>
    </details>
  )
}
