import { Check, PencilLine, X } from 'lucide-react'
import { type FormEvent, useEffect, useState } from 'react'
import { buttonClassName, Input } from '@/design-system'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** Inline rename: a pencil that opens a name field. An empty name restores
 *  the default one. */
export function ExecutionNameControl({
  executionId,
  fallbackLabel,
  label,
  onRename,
}: {
  executionId: string
  fallbackLabel: string
  label: string
  onRename: (executionId: string, label: string) => Promise<void>
}) {
  const [draft, setDraft] = useState(label)
  const [editing, setEditing] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => setDraft(label), [label])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setSaving(true)
    setError(null)
    try {
      await onRename(executionId, draft)
      setEditing(false)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }

  return (
    <span
      className="inline-flex flex-wrap items-center gap-2"
      data-rename-control
    >
      {editing ? (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(event) => void submit(event)}
        >
          <Input
            aria-label={`Name ${fallbackLabel}`}
            className="w-56"
            maxLength={80}
            placeholder={fallbackLabel}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <button
            className={buttonClassName({ variant: 'primary', size: 'compact' })}
            disabled={saving}
            aria-label={
              saving ? 'Saving execution name' : 'Save execution name'
            }
            title="Save execution name"
            type="submit"
          >
            <Check aria-hidden="true" size={14} />
          </button>
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            disabled={saving}
            aria-label="Cancel rename"
            title="Cancel rename"
            type="button"
            onClick={() => {
              setDraft(label)
              setError(null)
              setEditing(false)
            }}
          >
            <X aria-hidden="true" size={14} />
          </button>
        </form>
      ) : (
        <button
          aria-label={`Rename ${label.trim() || fallbackLabel}`}
          className={buttonClassName({ variant: 'quiet', size: 'compact' })}
          title={`Rename ${label.trim() || fallbackLabel}`}
          type="button"
          onClick={() => setEditing(true)}
        >
          <PencilLine aria-hidden="true" size={14} strokeWidth={1.8} />
        </button>
      )}
      {error ? (
        <small className="text-xs text-danger" role="alert">
          {error}
        </small>
      ) : null}
    </span>
  )
}
