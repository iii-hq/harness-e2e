import { useRef } from 'react'
import { buttonClassName, Dialog } from '@/design-system'

/**
 * Confirmation shown before unsaved edits are thrown away. Shared by the
 * new-plan page and the local test editor so both behave the same way
 * (audit PN-03 / NT-06). Escape, the backdrop and the close control all mean
 * "keep editing"; the safe action holds focus on open.
 */
export function DiscardDraftDialog({
  open,
  title = 'Discard draft changes?',
  warning,
  keepLabel = 'keep editing',
  discardLabel = 'discard and continue',
  onKeep,
  onDiscard,
}: {
  open: boolean
  title?: string
  warning: string
  keepLabel?: string
  discardLabel?: string
  onKeep: () => void
  onDiscard: () => void
}) {
  const keepRef = useRef<HTMLButtonElement>(null)
  if (!open) return null
  return (
    <Dialog
      open
      size="sm"
      onClose={onKeep}
      title={title}
      description={warning}
      closeLabel={keepLabel}
      className="harness-e2e-discard-dialog"
      initialFocus={keepRef}
      footer={
        <div className="ds-dialog-footer-actions">
          <button
            ref={keepRef}
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={onKeep}
          >
            {keepLabel}
          </button>
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="button"
            onClick={onDiscard}
          >
            {discardLabel}
          </button>
        </div>
      }
    />
  )
}
