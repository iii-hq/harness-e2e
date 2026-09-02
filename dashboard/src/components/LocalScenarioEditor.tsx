import { FileText, FileUp, Plus, Trash2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { DiscardDraftDialog } from '@/components/DiscardDraftDialog'
import { buttonClassName } from '@/design-system'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import {
  buildLocalScenarioSource,
  INITIAL_LOCAL_SCENARIO_DRAFT,
  LOCAL_SCENARIO_TEMPLATE,
  type LocalScenarioDraft,
  localScenarioDraftIssue,
  localScenarioValidationWeight,
  parseLocalScenarioSource,
} from '@/lib/local-scenario-authoring'

export { LOCAL_SCENARIO_TEMPLATE }

const MAX_LOCAL_SCENARIO_BYTES = 256 * 1024

/**
 * Audit NT-03: the editor starts empty. The template text is shown as
 * placeholders, so a test cannot be created without writing anything.
 */
export const EMPTY_LOCAL_SCENARIO_DRAFT: LocalScenarioDraft = {
  title: '',
  version: '1',
  beforeTest: '',
  prompt: '',
  validations: [
    { id: 'validation-1', title: '', weight: '', instructions: '' },
  ],
}

const PLACEHOLDER = INITIAL_LOCAL_SCENARIO_DRAFT

function cloneEmptyDraft(): LocalScenarioDraft {
  return {
    ...EMPTY_LOCAL_SCENARIO_DRAFT,
    validations: EMPTY_LOCAL_SCENARIO_DRAFT.validations.map((validation) => ({
      ...validation,
    })),
  }
}

/** Derives a Markdown file name from the test name, e.g. "Database recovery" → database-recovery.md. */
export function fileNameForTitle(title: string) {
  const slug = title
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return slug ? `${slug}.md` : ''
}

export function compiledLocalScenarioId(fileName: string) {
  const stem = fileName.replace(/\.md$/i, '')
  const normalized = stem
    .toLowerCase()
    .replace(/[- ]/g, '_')
    .replace(/[^a-z0-9_]/g, '')
  return normalized ? `local_${normalized}` : 'local_…'
}

function safeLocalFileName(fileName: string) {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9 _-]*\.md$/.test(fileName)) return false
  const id = compiledLocalScenarioId(fileName).replace(/^local_/, '')
  return id !== '' && !id.includes('__')
}

export function isLocalScenarioDraftDirty(
  draft: LocalScenarioDraft,
  fileNameTouched: boolean,
) {
  return (
    fileNameTouched ||
    JSON.stringify(draft) !== JSON.stringify(EMPTY_LOCAL_SCENARIO_DRAFT)
  )
}

/** Audit NT-09: shows how the total is reached, e.g. "70 + 30 = 100%". */
export function weightBreakdown(draft: LocalScenarioDraft) {
  const total = localScenarioValidationWeight(draft)
  const parts = draft.validations.map((validation) => {
    const weight = Number(validation.weight)
    return Number.isInteger(weight) && weight > 0 ? String(weight) : '0'
  })
  return parts.length > 1 ? `${parts.join(' + ')} = ${total}%` : `${total}%`
}

function responseScenarioId(value: Record<string, unknown>) {
  const scenario = value.scenario
  if (!scenario || typeof scenario !== 'object') return null
  const id = (scenario as Record<string, unknown>).scenario_id
  return typeof id === 'string' ? id : null
}

const fieldClassName =
  'min-h-11 w-full rounded-[6px] border border-[var(--color-rule)] bg-panel-raised px-3 text-sm text-ink placeholder:text-ink-muted focus-visible:border-[var(--color-rule-focus)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-rule-focus)] disabled:opacity-50'
const textareaClassName =
  'min-h-28 w-full resize-y rounded-[6px] border border-[var(--color-rule)] bg-panel-raised px-3 py-2.5 text-sm leading-6 text-ink placeholder:text-ink-muted focus-visible:border-[var(--color-rule-focus)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-rule-focus)] disabled:opacity-50'
const fieldLabelClassName = 'grid gap-2 text-xs font-semibold text-ink-soft'

export function LocalScenarioEditor({
  bridge,
  disabled = false,
  initialFileName = 'new-test.md',
  onClose,
  onCreated,
}: {
  bridge: DashboardDataBridge
  disabled?: boolean
  initialFileName?: string
  onClose: () => void
  onCreated: (scenarioId: string) => void
}) {
  const dialogRef = useRef<HTMLDialogElement>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  const nextValidationId = useRef(2)
  // null means "derived from the test name"; a string is a manual edit.
  const [fileNameInput, setFileNameInput] = useState<string | null>(null)
  const [draft, setDraft] = useState<LocalScenarioDraft>(cloneEmptyDraft)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [confirmingClose, setConfirmingClose] = useState(false)

  useEffect(() => {
    const dialog = dialogRef.current
    if (dialog && !dialog.open) dialog.showModal()
  }, [])

  const fileName =
    fileNameInput ?? (fileNameForTitle(draft.title) || initialFileName)
  const source = useMemo(() => buildLocalScenarioSource(draft), [draft])
  const weight = useMemo(() => localScenarioValidationWeight(draft), [draft])
  const draftIssue = useMemo(() => localScenarioDraftIssue(draft), [draft])
  const safeFileName = safeLocalFileName(fileName.trim())
  const busy = disabled || saving
  const canCreate = safeFileName && draftIssue === null && !busy
  const dirty = isLocalScenarioDraftDirty(draft, fileNameInput !== null)

  const updateDraft = <Key extends keyof LocalScenarioDraft>(
    key: Key,
    value: LocalScenarioDraft[Key],
  ) => {
    setDraft((current) => ({ ...current, [key]: value }))
    setError(null)
  }

  const updateValidation = (
    id: string,
    key: 'title' | 'weight' | 'instructions',
    value: string,
  ) => {
    setDraft((current) => ({
      ...current,
      validations: current.validations.map((validation) =>
        validation.id === id ? { ...validation, [key]: value } : validation,
      ),
    }))
    setError(null)
  }

  const addValidation = () => {
    const id = `validation-${nextValidationId.current}`
    nextValidationId.current += 1
    setDraft((current) => ({
      ...current,
      validations: [
        ...current.validations,
        { id, title: '', weight: '', instructions: '' },
      ],
    }))
    setError(null)
  }

  const removeValidation = (id: string) => {
    setDraft((current) => ({
      ...current,
      validations: current.validations.filter(
        (validation) => validation.id !== id,
      ),
    }))
    setError(null)
  }

  // Audit NT-06: Escape and the close control ask before dropping edits.
  const requestClose = () => {
    if (saving) return
    if (dirty) setConfirmingClose(true)
    else onClose()
  }

  const save = async () => {
    if (!safeFileName || draftIssue) {
      setError(
        !safeFileName
          ? 'Use a safe Markdown file name before creating the test.'
          : draftIssue,
      )
      return
    }
    setSaving(true)
    setError(null)
    try {
      const response = await bridge.createLocalScenario({
        file_name: fileName.trim(),
        source,
      })
      const scenarioId = responseScenarioId(response)
      if (!scenarioId)
        throw new Error('The worker did not return a scenario id.')
      onCreated(scenarioId)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  const importFile = async (file: File | undefined) => {
    if (!file) return
    setError(null)
    setNotice(null)
    try {
      if (file.size > MAX_LOCAL_SCENARIO_BYTES)
        throw new Error('The Markdown file exceeds the 256 KiB limit.')
      const imported = parseLocalScenarioSource(await file.text())
      nextValidationId.current = imported.validations.length + 1
      setFileNameInput(file.name)
      setDraft(imported)
      setNotice(`Imported ${file.name}. Review the fields before creating.`)
    } catch (cause) {
      setError(
        `Could not import ${file.name}: ${cause instanceof Error ? cause.message : String(cause)}`,
      )
    } finally {
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  const statusMessage = error ? (
    <span className="text-danger">{error}</span>
  ) : !safeFileName ? (
    <span className="text-ink-muted">
      Enter a safe Markdown file name to continue.
    </span>
  ) : draftIssue ? (
    <span className="text-ink-muted">{draftIssue}</span>
  ) : (
    <span className="text-success">
      Ready to save locally. No execution starts.
    </span>
  )

  return (
    <dialog
      ref={dialogRef}
      className="ds-root m-auto hidden max-h-[94dvh] w-[min(1120px,calc(100%_-_1rem))] max-w-none overflow-hidden rounded-[6px] border border-[var(--color-edge)] bg-panel p-0 text-ink shadow-[var(--shadow-panel)] open:grid open:grid-rows-[auto_minmax(0,1fr)_auto] backdrop:bg-[var(--color-backdrop)] backdrop:backdrop-blur-sm sm:w-[min(1120px,calc(100%_-_2rem))] max-[560px]:m-0 max-[560px]:h-dvh max-[560px]:max-h-dvh max-[560px]:w-screen max-[560px]:rounded-none max-[560px]:border-0"
      onCancel={(event) => {
        event.preventDefault()
        requestClose()
      }}
      aria-labelledby="local-scenario-editor-title"
      aria-describedby="local-scenario-editor-description"
    >
      <header className="flex items-start justify-between gap-5 border-b border-[var(--color-rule)] bg-panel px-5 py-4 sm:px-6">
        <div className="min-w-0 max-w-3xl">
          <h2
            className="m-0 text-base font-semibold tracking-[-0.02em] text-ink"
            id="local-scenario-editor-title"
          >
            Create a local test
          </h2>
          <p
            className="mt-1 mb-0 text-xs leading-5 text-ink-muted"
            id="local-scenario-editor-description"
          >
            Fill in the test definition or import an existing Markdown file. The
            Markdown is saved outside this repository without starting an
            execution.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            type="button"
            onClick={() => fileRef.current?.click()}
            disabled={busy}
          >
            <FileUp size={15} aria-hidden="true" />
            <span className="max-[460px]:sr-only">import .md</span>
          </button>
          <button
            className="inline-grid size-11 shrink-0 place-items-center rounded-[6px] border-0 bg-transparent text-ink-soft hover:bg-panel-raised hover:text-ink disabled:opacity-50"
            type="button"
            onClick={requestClose}
            disabled={saving}
            aria-label="Close local test creation"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </div>
      </header>

      <div className="min-h-0 overflow-auto">
        <form
          id="local-scenario-form"
          className="grid min-w-0 gap-px bg-[var(--color-rule)] lg:grid-cols-12"
          onSubmit={(event) => {
            event.preventDefault()
            void save()
          }}
        >
          <input
            ref={fileRef}
            className="hidden"
            type="file"
            accept=".md,text/markdown,text/plain"
            onChange={(event) => void importFile(event.target.files?.[0])}
            disabled={busy}
          />

          <div className="grid min-w-0 content-start gap-6 bg-panel p-5 sm:p-6 lg:col-span-8">
            {notice ? (
              <p
                className="m-0 flex items-center gap-2 rounded-[6px] bg-[var(--surface-soft)] px-3 py-2 text-xs text-ink-soft"
                role="status"
              >
                <FileText size={14} aria-hidden="true" />
                {notice}
              </p>
            ) : null}

            <section
              className="grid gap-4"
              aria-labelledby="test-details-title"
            >
              <div>
                <h3
                  className="m-0 text-sm font-semibold text-ink"
                  id="test-details-title"
                >
                  Test details
                </h3>
                <p className="mt-1 mb-0 text-xs leading-5 text-ink-muted">
                  The name becomes the title and, unless you edit it, the
                  Markdown file name.
                </p>
              </div>
              <label className={fieldLabelClassName}>
                Test name
                <input
                  className={fieldClassName}
                  value={draft.title}
                  placeholder="Database recovery"
                  onChange={(event) => updateDraft('title', event.target.value)}
                  disabled={busy}
                  autoFocus
                  required
                />
              </label>
              <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_8rem]">
                <label className={fieldLabelClassName}>
                  Markdown file name
                  <input
                    className={`${fieldClassName} font-mono text-xs`}
                    value={fileName}
                    onChange={(event) => {
                      setFileNameInput(event.target.value)
                      setError(null)
                    }}
                    disabled={busy}
                    spellCheck={false}
                    aria-invalid={!safeFileName}
                    aria-describedby="local-file-name-help"
                    required
                  />
                  <small
                    className={`text-xs font-normal leading-4 ${safeFileName ? 'text-ink-muted' : 'text-danger'}`}
                    id="local-file-name-help"
                  >
                    {safeFileName
                      ? `Compiles to ${compiledLocalScenarioId(fileName.trim())}`
                      : 'Use letters, numbers, spaces, hyphens or underscores and end with .md.'}
                  </small>
                </label>
                <label className={`${fieldLabelClassName} content-start`}>
                  Version
                  <input
                    className={`${fieldClassName} font-mono text-xs`}
                    type="number"
                    min="1"
                    step="1"
                    inputMode="numeric"
                    value={draft.version}
                    onChange={(event) =>
                      updateDraft('version', event.target.value)
                    }
                    disabled={busy}
                    required
                  />
                </label>
              </div>
            </section>

            <section
              className="grid gap-4 border-t border-[var(--color-rule)] pt-6"
              aria-labelledby="test-instructions-title"
            >
              <div>
                <h3
                  className="m-0 text-sm font-semibold text-ink"
                  id="test-instructions-title"
                >
                  Instructions
                </h3>
                <p className="mt-1 mb-0 text-xs leading-5 text-ink-muted">
                  Describe the isolated setup first, then the task the Harness
                  must complete.
                </p>
              </div>
              <label className={fieldLabelClassName}>
                Before test
                <textarea
                  className={textareaClassName}
                  value={draft.beforeTest}
                  placeholder={PLACEHOLDER.beforeTest}
                  onChange={(event) =>
                    updateDraft('beforeTest', event.target.value)
                  }
                  disabled={busy}
                  required
                />
              </label>
              <label className={fieldLabelClassName}>
                Task prompt
                <textarea
                  className={textareaClassName}
                  value={draft.prompt}
                  placeholder={PLACEHOLDER.prompt}
                  onChange={(event) =>
                    updateDraft('prompt', event.target.value)
                  }
                  disabled={busy}
                  required
                />
              </label>
            </section>

            <section
              className="grid gap-4 border-t border-[var(--color-rule)] pt-6"
              aria-labelledby="validation-criteria-title"
            >
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <h3
                    className="m-0 text-sm font-semibold text-ink"
                    id="validation-criteria-title"
                  >
                    Validation criteria
                  </h3>
                  <p className="mt-1 mb-0 text-xs leading-5 text-ink-muted">
                    Describe the evidence for each criterion. Weights must add
                    up to exactly 100%.
                  </p>
                </div>
                <button
                  className={buttonClassName({
                    variant: 'secondary',
                    size: 'compact',
                  })}
                  type="button"
                  onClick={addValidation}
                  disabled={busy}
                >
                  <Plus size={14} aria-hidden="true" />
                  add criterion
                </button>
              </div>

              <div className="grid gap-3">
                {draft.validations.map((validation, index) => {
                  const placeholder =
                    PLACEHOLDER.validations[index] ??
                    PLACEHOLDER.validations[PLACEHOLDER.validations.length - 1]
                  return (
                    <fieldset
                      className="m-0 grid gap-3 rounded-[6px] border border-[var(--color-rule)] bg-panel-raised p-4"
                      key={validation.id}
                    >
                      <legend className="sr-only">
                        Validation {index + 1}
                      </legend>
                      <div className="flex items-center justify-between gap-3">
                        <strong className="font-mono text-xs font-semibold text-ink-soft">
                          Criterion {index + 1}
                        </strong>
                        <button
                          className={buttonClassName({
                            variant: 'quiet',
                            size: 'compact',
                          })}
                          type="button"
                          onClick={() => removeValidation(validation.id)}
                          disabled={busy || draft.validations.length === 1}
                          aria-label={`Remove validation ${index + 1}`}
                        >
                          <Trash2 size={13} aria-hidden="true" />
                          remove
                        </button>
                      </div>
                      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem]">
                        <label className={fieldLabelClassName}>
                          Name
                          <input
                            className={fieldClassName}
                            value={validation.title}
                            placeholder={placeholder.title}
                            onChange={(event) =>
                              updateValidation(
                                validation.id,
                                'title',
                                event.target.value,
                              )
                            }
                            disabled={busy}
                            required
                          />
                        </label>
                        <label className={fieldLabelClassName}>
                          Weight %
                          <input
                            className={`${fieldClassName} font-mono text-xs`}
                            type="number"
                            min="1"
                            max="100"
                            step="1"
                            inputMode="numeric"
                            value={validation.weight}
                            placeholder={placeholder.weight}
                            onChange={(event) =>
                              updateValidation(
                                validation.id,
                                'weight',
                                event.target.value,
                              )
                            }
                            disabled={busy}
                            required
                          />
                        </label>
                      </div>
                      <label className={fieldLabelClassName}>
                        Evaluation instructions
                        <textarea
                          className={`${textareaClassName} min-h-24`}
                          value={validation.instructions}
                          placeholder={placeholder.instructions}
                          onChange={(event) =>
                            updateValidation(
                              validation.id,
                              'instructions',
                              event.target.value,
                            )
                          }
                          disabled={busy}
                          required
                        />
                      </label>
                    </fieldset>
                  )
                })}
              </div>
            </section>
          </div>

          <aside className="grid min-w-0 content-start gap-5 bg-panel-raised p-5 sm:p-6 lg:sticky lg:top-0 lg:col-span-4">
            <div className="grid gap-2">
              <span className="text-xs font-semibold text-ink-soft">
                Generated Markdown
              </span>
              <code className="break-all rounded-[6px] bg-panel px-3 py-2 font-mono text-xs text-ink">
                {fileName.trim() || initialFileName}
              </code>
              <p className="m-0 text-xs leading-5 text-ink-muted">
                Plan <code>local</code> · {draft.validations.length} validation
                {draft.validations.length === 1 ? '' : 's'} · no execution
              </p>
            </div>

            <details className="group rounded-[6px] bg-panel p-3">
              <summary className="cursor-pointer text-xs font-semibold text-ink-soft">
                Preview Markdown
              </summary>
              <pre className="mt-3 mb-0 max-h-72 overflow-auto whitespace-pre-wrap break-words border-t border-[var(--color-rule)] pt-3 font-mono text-xs leading-5 text-ink-soft">
                {source}
              </pre>
            </details>
          </aside>
        </form>
      </div>

      {/* Audit NT-01: the state line and the actions stay visible at every
          width instead of sitting after the last criterion. */}
      <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--color-rule)] bg-panel px-5 py-3 sm:px-6">
        <p
          className="m-0 min-w-0 flex-1 text-xs leading-5"
          role="status"
          aria-live="polite"
        >
          {statusMessage}
        </p>
        <div className="flex flex-wrap items-center gap-3">
          <output
            className={`font-mono text-xs tabular-nums ${weight === 100 ? 'text-success' : 'text-warning'}`}
            aria-label="Validation weight total"
          >
            {weightBreakdown(draft)}
          </output>
          <button
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={requestClose}
            disabled={saving}
          >
            cancel
          </button>
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="submit"
            form="local-scenario-form"
            disabled={!canCreate}
            aria-busy={saving}
          >
            <Plus size={15} aria-hidden="true" />
            {saving ? 'creating test…' : 'create test'}
          </button>
        </div>
      </footer>

      <DiscardDraftDialog
        open={confirmingClose}
        title="Discard this test?"
        warning="The definition has not been saved. Closing now throws away what you typed."
        discardLabel="discard test"
        onKeep={() => setConfirmingClose(false)}
        onDiscard={() => {
          setConfirmingClose(false)
          onClose()
        }}
      />
    </dialog>
  )
}
