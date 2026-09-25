import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  StatusDot,
} from '@iii-dev/console-ui'
import { MoreHorizontal } from 'lucide-react'
import {
  Fragment,
  type HTMLAttributes,
  type ReactNode,
  type SyntheticEvent,
} from 'react'
import { RESULT_STATES, type ResultState } from '@/lib/result-status'
import './styles.css'

// The pieces every redesigned screen shares: the result dot and label, the
// fact chip and the row's ⋯ menu. They wrap the host's own components where
// the host has one (StatusDot, DropdownMenu).

function classes(...values: Array<string | false | null | undefined>) {
  return values.filter(Boolean).join(' ')
}

export type StatusLabelProps = HTMLAttributes<HTMLSpanElement> & {
  state: ResultState
  /** Replaces the state's label (`Cancelling`); the tone stays the state's. */
  label?: string
  /** Paints the label by its tone too, as the execution detail does: alert
   *  states in strong alert, ghost states (queued, cancelled) faint. */
  tinted?: boolean
}

/** A 6px dot and a label; the dot pulses only while the state is live. */
export function StatusLabel({
  state,
  label,
  tinted = false,
  className,
  ...props
}: StatusLabelProps) {
  const presentation = RESULT_STATES[state]
  const { tone } = presentation
  return (
    <span
      className={classes('ds-status-label', className)}
      data-state={state}
      data-tone={tone}
      data-tinted={tinted || undefined}
      {...props}
    >
      {/* The host dot has no ghost tone: the label paints it (common.css). */}
      <StatusDot
        className="ds-status-label-dot"
        tone={tone === 'ghost' ? 'ink' : tone}
        pulse={presentation.live}
      />
      <span>{label ?? presentation.label}</span>
    </span>
  )
}

export type FactChipProps = HTMLAttributes<HTMLSpanElement> & {
  label: string
  value: string
  /** The whole value when `value` is a short form (an image tag, an id). */
  full?: string
}

/** A label and a mono value; the title carries the whole value. */
export function FactChip({
  label,
  value,
  full,
  className,
  ...props
}: FactChipProps) {
  return (
    <span
      className={classes('ds-fact', className)}
      title={`${label}: ${full ?? value}`}
      {...props}
    >
      <span className="ds-fact-label">{label}</span>
      <span className="ds-fact-value">{value}</span>
    </span>
  )
}

/** Fact chips in a row that wraps. */
export function FactList({
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  return <div className={classes('ds-fact-list', className)} {...props} />
}

export type RowMenuItem = {
  label: string
  /** A second line under the label. */
  hint?: string
  icon?: ReactNode
  onSelect?: () => void
  /** Destructive: drawn in the alert color. */
  danger?: boolean
  /** Why the item cannot run now. Disables it and shows as its hint; the
   *  item stays focusable so a screen reader reads the reason. */
  disabledReason?: string
  /** Draws a separator above the item. */
  separator?: boolean
}

export type RowMenuProps = {
  /** Names the button and the menu: `Actions for Regression`. */
  label: string
  items: RowMenuItem[]
  className?: string
}

// The menu sits in a row that may open on click or Enter; neither the
// button nor the (portalled, but still React-nested) menu lets them through.
function stop(event: SyntheticEvent) {
  event.stopPropagation()
}

/** The row's ⋯ button and its menu, on the host's DropdownMenu. */
export function RowMenu({ label, items, className }: RowMenuProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className={classes('ds-row-menu-trigger', className)}
          aria-label={label}
          onClick={stop}
          onKeyDown={stop}
        >
          <MoreHorizontal size={16} aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        aria-label={label}
        className="ds-row-menu"
        onClick={stop}
        onKeyDown={stop}
      >
        {items.map((item) => {
          const hint = item.disabledReason ?? item.hint
          // Not Radix's `disabled`, which drops the item from focus and dims
          // the reason with it: aria-disabled, and selecting does nothing.
          const disabled = Boolean(item.disabledReason)
          return (
            <Fragment key={item.label}>
              {item.separator ? <DropdownMenuSeparator /> : null}
              <DropdownMenuItem
                className="ds-row-menu-item"
                data-danger={item.danger || undefined}
                aria-disabled={disabled || undefined}
                onSelect={
                  disabled ? (event) => event.preventDefault() : item.onSelect
                }
              >
                <span className="ds-row-menu-icon" aria-hidden="true">
                  {item.icon}
                </span>
                <span className="ds-row-menu-text">
                  <span className="ds-row-menu-label">{item.label}</span>
                  {hint ? (
                    <span className="ds-row-menu-hint">{hint}</span>
                  ) : null}
                </span>
              </DropdownMenuItem>
            </Fragment>
          )
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
