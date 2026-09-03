import { useEffect, useRef } from 'react'
import { buttonClassName } from '@/design-system'

/**
 * Confirmation shown before unsaved edits are thrown away. Shared by the
 * new-plan page and the local test editor so both behave the same way
 * (audit PN-03 / NT-06): a real modal with backdrop; Escape and a backdrop
 * click both mean "keep editing".
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
  const dialogRef = useRef<HTMLDialogElement>(null)
  const keepRef = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    const dialog = dialogRef.current
    if (!dialog) return
    if (open && !dialog.open) {
      dialog.showModal()
      keepRef.current?.focus()
    }
    if (!open && dialog.open) dialog.close()
  }, [open])
  if (!open) return null
  return (
    <dialog
      ref={dialogRef}
      className="harness-e2e-discard-dialog m-auto w-[min(28rem,calc(100vw-2rem))] rounded-[6px] border border-line bg-panel p-5 text-ink backdrop:bg-app-backdrop backdrop:backdrop-blur-[5px]"
      aria-labelledby="discard-draft-title"
      aria-describedby="discard-draft-description"
      onCancel={(event) => {
        event.preventDefault()
        onKeep()
      }}
      onKeyDown={(event) => {
        // Chromium groups a modal opened from an Escape press with the one
        // below it, so its cancel event can land on the parent. Handle the
        // key here, before the close watcher sees it.
        if (event.key !== 'Escape') return
        event.preventDefault()
        event.stopPropagation()
        onKeep()
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onKeep()
      }}
    >
      <h2 id="discard-draft-title" className="m-0 text-base font-semibold">
        {title}
      </h2>
      <p
        id="discard-draft-description"
        className="m-0 mt-2 text-[13px] leading-[1.5] text-ink-soft"
      >
        {warning}
      </p>
      <div className="harness-e2e-discard-dialog-actions mt-4 flex flex-wrap justify-end gap-2">
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
    </dialog>
  )
}
