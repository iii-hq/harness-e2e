import { useCallback, useEffect, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { ProviderCredentials } from '@/components/ProviderCredentials'
import {
  buttonClassName,
  Callout,
  DataTable,
  Dialog,
  EmptyState,
  Field,
  fieldDescribedBy,
  Input,
  PageHeader,
  Textarea,
} from '@/design-system'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type Stack,
} from '@/lib/dashboard-data-source'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`
}

/** What a stack installs, in one line. */
function stackScope(stack: Stack) {
  return [
    `iii ${stack.iii ?? '—'}`,
    stack.template ? `template ${stack.template}` : null,
    plural(stack.containers.length, 'container', 'containers'),
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Its containers with the version or commit each pins. */
function containerPins(stack: Stack) {
  return stack.containers
    .map(({ name, version, commit }) =>
      commit
        ? `${name} @${commit.slice(0, 12)}`
        : version
          ? `${name} ${version}`
          : name,
    )
    .join(', ')
}

function Warnings({ warnings }: { warnings: string[] }) {
  if (warnings.length === 0) return null
  return (
    <Callout
      tone="warning"
      title={plural(warnings.length, 'warning', 'warnings')}
      data-stack-warnings
    >
      <ul className="m-0 grid list-disc gap-1 pl-4">
        {warnings.map((warning, index) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: warnings repeat and never reorder
          <li key={index}>{warning}</li>
        ))}
      </ul>
    </Callout>
  )
}

/** Edits a stack of this Console: its name and its YAML, as written. Saving
 *  keeps the editor open with the warnings of what was saved. A repository
 *  stack opens read-only. */
function StackEditor({
  bridge,
  stack,
  onClose,
  onSaved,
}: {
  bridge: DashboardDataBridge
  stack: Stack
  onClose: () => void
  onSaved: () => void
}) {
  const [label, setLabel] = useState(stack.label)
  const [yaml, setYaml] = useState(stack.yaml)
  const [saved, setSaved] = useState(stack)
  const [attempted, setAttempted] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [status, setStatus] = useState('')
  const readOnly = stack.source === 'repository'
  const labelError =
    attempted && label.trim() === '' ? 'Name the stack.' : undefined
  const changed = label.trim() !== saved.label || yaml !== saved.yaml

  const save = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (label.trim() === '') {
      setAttempted(true)
      document.getElementById('stack-editor-label')?.focus()
      return
    }
    setSaving(true)
    setError(null)
    try {
      const next = await bridge.updateStack(stack.id, {
        label: label.trim(),
        yaml,
      })
      setSaved(next)
      setStatus(
        next.warnings.length
          ? `saved with ${plural(next.warnings.length, 'warning', 'warnings')}`
          : 'saved',
      )
      onSaved()
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog
      open
      onClose={() => !saving && onClose()}
      size="lg"
      tall
      kicker={readOnly ? 'Repository stack' : 'Stack'}
      title={readOnly ? `View ${stack.label}` : `Edit ${stack.label}`}
      description={
        readOnly
          ? 'A stack of the repository, read-only: copy it to edit a stack of this Console.'
          : 'Where a suite runs: an iii Compose project plus iii and an optional template, as in stacks/*.yaml. Only YAML that does not parse, or no containers, is refused; everything else is a warning.'
      }
      closeLabel="Close stack editor"
      className="ds-root"
      footer={
        <div className="flex flex-wrap items-center justify-end gap-2">
          <span className="mr-auto text-xs text-ink-soft" role="status">
            {saving ? '' : changed ? 'unsaved changes' : status}
          </span>
          <button
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={onClose}
            disabled={saving}
          >
            close
          </button>
          {readOnly ? null : (
            <button
              className={buttonClassName({ variant: 'primary' })}
              type="submit"
              form="stack-editor-form"
              disabled={saving}
              aria-busy={saving}
            >
              {saving ? 'saving…' : 'save stack'}
            </button>
          )}
        </div>
      }
    >
      <form
        id="stack-editor-form"
        className="grid min-w-0 gap-4"
        onSubmit={save}
        noValidate
      >
        {readOnly ? null : (
          <Field
            label="Stack name"
            htmlFor="stack-editor-label"
            meta="required"
            error={labelError}
          >
            <Input
              id="stack-editor-label"
              value={label}
              maxLength={160}
              aria-invalid={labelError ? true : undefined}
              aria-describedby={fieldDescribedBy('stack-editor-label', {
                error: Boolean(labelError),
              })}
              onChange={(event) => setLabel(event.target.value)}
              disabled={saving}
            />
          </Field>
        )}
        <Warnings warnings={saved.warnings} />
        <Field
          label="Stack YAML"
          htmlFor="stack-editor-yaml"
          hint={
            readOnly
              ? 'As stacks/ in this runner writes it.'
              : 'Kept exactly as written, comments included.'
          }
          error={error}
        >
          <Textarea
            id="stack-editor-yaml"
            className="min-h-[24rem] font-mono text-xs leading-5"
            value={yaml}
            readOnly={readOnly}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-invalid={error ? true : undefined}
            aria-describedby={fieldDescribedBy('stack-editor-yaml', {
              hint: true,
              error: Boolean(error),
            })}
            onChange={(event) => setYaml(event.target.value)}
            disabled={saving}
          />
        </Field>
      </form>
    </Dialog>
  )
}

/** Stacks: the repository's, read-only, and this Console's. Any stack can
 *  be copied into one of this Console to edit. Below them, the provider
 *  credentials a Docker execution's stack receives. */
export function StacksPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [stacks, setStacks] = useState<Stack[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [editing, setEditing] = useState<Stack | null>(null)
  const [deleting, setDeleting] = useState<Stack | null>(null)

  const load = useCallback(async () => {
    try {
      const next = await getDashboardDataBridge()
      setBridge(next)
      setStacks((await next.listStacks()).stacks)
      setError(null)
    } catch (cause) {
      setError(errorText(cause))
    }
  }, [])
  useEffect(() => {
    void load()
  }, [load])

  const copy = async (stack: Stack) => {
    if (!bridge) return
    setBusy(stack.id)
    setError(null)
    try {
      const created = await bridge.createStack(stack.id)
      await load()
      setEditing(created)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setBusy(null)
    }
  }
  const remove = async (stack: Stack) => {
    if (!bridge) return
    setBusy(stack.id)
    setError(null)
    try {
      await bridge.deleteStack(stack.id)
      setDeleting(null)
      await load()
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions active="stacks" />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="stacks"
          summary="Where a suite runs: an iii Compose project, the iii release and an optional template. The repository's stacks are read-only; copy one to edit it here."
        />
        {error ? (
          <p className="mt-4 text-sm text-danger" role="alert">
            {error}
          </p>
        ) : null}
        {stacks === null ? (
          error ? (
            <EmptyState
              className="mt-6"
              tone="error"
              title="Stacks could not be loaded"
              description={error}
              actions={
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => void load()}
                >
                  try again
                </button>
              }
            />
          ) : (
            <div className="mt-6 grid gap-2" aria-busy="true" role="status">
              <span className="ds-visually-hidden">Loading stacks</span>
              {['first', 'second', 'third'].map((placeholder) => (
                <div
                  key={placeholder}
                  className="h-12 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
                />
              ))}
            </div>
          )
        ) : (
          <div className="mt-6" data-stacks>
            <DataTable caption="Stacks" collapse minWidth="52rem">
              <thead>
                <tr>
                  <th scope="col">Stack</th>
                  <th scope="col">Source</th>
                  <th scope="col">Declares</th>
                  <th scope="col">Warnings</th>
                  <th scope="col">
                    <span className="ds-visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {stacks.map((stack) => (
                  <tr key={stack.id} data-stack={stack.id}>
                    <td data-label="Stack">
                      <span className="grid gap-0.5">
                        <strong className="text-sm font-semibold text-ink">
                          {stack.label}
                        </strong>
                        <span className="font-mono text-xs text-ink-muted">
                          {stack.id}
                        </span>
                      </span>
                    </td>
                    <td data-label="Source" className="text-xs text-ink-soft">
                      {stack.source === 'local' ? 'this Console' : 'repository'}
                    </td>
                    <td data-label="Declares" className="text-xs text-ink">
                      <span className="grid gap-0.5">
                        <span>{stackScope(stack)}</span>
                        <span className="max-w-[32rem] font-mono text-ink-muted">
                          {containerPins(stack)}
                        </span>
                      </span>
                    </td>
                    <td data-label="Warnings" className="text-xs">
                      {stack.warnings.length ? (
                        <ul className="m-0 grid max-w-[28rem] list-none gap-1 p-0 text-warning">
                          {stack.warnings.map((warning, index) => (
                            // biome-ignore lint/suspicious/noArrayIndexKey: warnings repeat and never reorder
                            <li key={index}>{warning}</li>
                          ))}
                        </ul>
                      ) : (
                        <span className="text-ink-muted">—</span>
                      )}
                    </td>
                    <td className="text-right">
                      <span className="inline-flex flex-wrap justify-end gap-2">
                        <button
                          className={buttonClassName({
                            variant: 'secondary',
                            size: 'compact',
                          })}
                          type="button"
                          aria-label={`Copy ${stack.label}`}
                          disabled={!bridge || busy !== null}
                          onClick={() => void copy(stack)}
                        >
                          copy
                        </button>
                        {stack.source === 'repository' ? (
                          <button
                            className={buttonClassName({
                              variant: 'secondary',
                              size: 'compact',
                            })}
                            type="button"
                            aria-label={`View ${stack.label}`}
                            disabled={!bridge || busy !== null}
                            onClick={() => setEditing(stack)}
                          >
                            view
                          </button>
                        ) : (
                          <>
                            <button
                              className={buttonClassName({
                                variant: 'secondary',
                                size: 'compact',
                              })}
                              type="button"
                              aria-label={`Edit ${stack.label}`}
                              disabled={!bridge || busy !== null}
                              onClick={() => setEditing(stack)}
                            >
                              edit
                            </button>
                            <button
                              className={buttonClassName({
                                variant: 'quiet',
                                size: 'compact',
                              })}
                              type="button"
                              aria-label={`Delete ${stack.label}`}
                              disabled={!bridge || busy !== null}
                              onClick={() => setDeleting(stack)}
                            >
                              delete
                            </button>
                          </>
                        )}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </div>
        )}
        <ProviderCredentials bridge={bridge} />
      </div>
      {editing && bridge ? (
        <StackEditor
          key={editing.id}
          bridge={bridge}
          stack={editing}
          onClose={() => setEditing(null)}
          onSaved={() => void load()}
        />
      ) : null}
      <Dialog
        open={deleting !== null}
        onClose={() => busy === null && setDeleting(null)}
        size="sm"
        title={`Delete ${deleting?.label ?? 'stack'}?`}
        description="Only this Console's copy goes; the repository's stacks stay."
        footer={
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              disabled={busy !== null}
              onClick={() => setDeleting(null)}
            >
              cancel
            </button>
            <button
              type="button"
              className={buttonClassName({ variant: 'primary' })}
              disabled={busy !== null}
              aria-busy={busy !== null}
              onClick={() => deleting && void remove(deleting)}
            >
              {busy !== null ? 'deleting…' : 'delete stack'}
            </button>
          </div>
        }
      />
    </div>
  )
}
