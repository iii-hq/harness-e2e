import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@iii-dev/console-ui'
import { Ellipsis } from 'lucide-react'

/** One of a section's actions (Run tests, Import from GitHub, …). */
export type HeaderAction = {
  id: string
  label: string
  /** The section's one primary action. */
  primary?: boolean
  disabled?: boolean
  /** A link, or … */
  href?: string
  /** … a command. */
  onSelect?: () => void
}

function run(action: HeaderAction) {
  if (action.href) window.location.hash = action.href.replace(/^#/, '')
  action.onSelect?.()
}

function ActionButton({ action }: { action: HeaderAction }) {
  const className = action.primary
    ? 'harness-e2e-header-action harness-e2e-header-action-primary'
    : 'harness-e2e-header-action harness-e2e-header-action-secondary'
  return action.href && !action.disabled ? (
    <a className={className} href={action.href} onClick={action.onSelect}>
      {action.label}
    </a>
  ) : (
    <button
      type="button"
      className={className}
      disabled={action.disabled}
      onClick={action.onSelect}
    >
      {action.label}
    </button>
  )
}

/** The section's actions in the Console header. Wide: every action (ghost +
 *  one primary). Below 720: the primary plus ⋯ holding the rest with full
 *  labels. Phone: ⋯ only; the primary pins to the bottom (PinnedPrimary). */
export function HeaderActions({
  actions,
  label = 'Page actions',
  narrow,
  phone,
}: {
  actions: HeaderAction[]
  label?: string
  narrow: boolean
  phone: boolean
}) {
  if (actions.length === 0) return null
  const primary = actions.find((action) => action.primary)
  const folded = narrow || phone
  const inHeader = folded ? (phone ? [] : primary ? [primary] : []) : actions
  const inMenu = folded
    ? actions.filter((action) => phone || action !== primary)
    : []
  return (
    // biome-ignore lint/a11y/useSemanticElements: a labelled group of header actions
    <div className="harness-e2e-header-actions" role="group" aria-label={label}>
      {inMenu.length > 0 ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="harness-e2e-header-action harness-e2e-header-action-icon"
              aria-label="More actions"
            >
              <Ellipsis size={16} aria-hidden="true" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {inMenu.map((action) => (
              <DropdownMenuItem
                key={action.id}
                disabled={action.disabled}
                onSelect={() => run(action)}
              >
                {action.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
      {inHeader.map((action) => (
        <ActionButton key={action.id} action={action} />
      ))}
    </div>
  )
}

/** On a phone the section's primary action pins to the bottom of the pane. */
export function PinnedPrimary({
  actions,
  phone,
}: {
  actions: HeaderAction[]
  phone: boolean
}) {
  const primary = actions.find((action) => action.primary)
  if (!phone || !primary) return null
  return (
    <div className="harness-e2e-pinned-primary">
      <ActionButton action={primary} />
    </div>
  )
}
