import { FileText, FileUp, Plus, Trash2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
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

function compiledId(fileName: string) {
  const stem = fileName.replace(/\.md$/i, '')
  const normalized = stem
    .toLowerCase()
    .replace(/[- ]/g, '_')
    .replace(/[^a-z0-9_]/g, '')
  return normalized ? `local_${normalized}` : 'local_…'
}

function safeLocalFileName(fileName: string) {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9 _-]*\.md$/.test(fileName)) return false
  const id = compiledId(fileName).replace(/^local_/, '')
  return id !== '' && !id.includes('__')
}

function responseScenarioId(value: Record<string, unknown>) {
  const scenario = value.scenario
  if (!scenario || typeof scenario !== 'object') return null
  const id = (scenario as Record<string, unknown>).scenario_id
  return typeof id === 'string' ? id : null
}

function fieldClassName() {
  return 'min-h-11 w-full rounded-lg border border-[var(--color-rule)] bg-panel-raised px-3 text-sm text-ink focus-visible:border-[var(--color-rule-focus)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-rule-focus)] disabled:opacity-50'
}

function textareaClassName() {
  return 'min-h-28 w-full resize-y rounded-lg border border-[var(--color-rule)] bg-panel-raised px-3 py-2.5 text-sm leading-6 text-ink focus-visible:border-[var(--color-rule-focus)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-rule-focus)] disabled:opacity-50'
}

function cloneInitialDraft(): LocalScenarioDraft {
  return {
    ...INITIAL_LOCAL_SCENARIO_DRAFT,
    validations: INITIAL_LOCAL_SCENARIO_DRAFT.validations.map((validation) => ({
      ...validation,
    })),
  }
}

export function LocalScenarioEditor({
  bridge,
  disabled = false,
  initialFileName = 'local-scenario.md',
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
  const nextValidationId = useRef(3)
  const [fileName, setFileName] = useState(initialFileName)
  const [draft, setDraft] = useState<LocalScenarioDraft>(cloneInitialDraft)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    const dialog = dialogRef.current
    if (dialog && !dialog.open) dialog.showModal()
  }, [])

  const source = useMemo(() => buildLocalScenarioSource(draft), [draft])
  const weight = useMemo(() => localScenarioValidationWeight(draft), [draft])
  const draftIssue = useMemo(() => localScenarioDraftIssue(draft), [draft])
  const safeFileName = safeLocalFileName(fileName.trim())
  const busy = disabled || saving
  const canCreate = safeFileName && draftIssue === null && !busy

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
      setFileName(file.name)
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

  return (
    <dialog
      ref={dialogRef}
      className="ds-root m-auto hidden max-h-[94dvh] w-[min(1120px,calc(100%_-_1rem))] max-w-none overflow-hidden rounded-xl border border-[var(--color-edge)] bg-panel p-0 text-ink shadow-[var(--shadow-panel)] open:grid open:grid-rows-[auto_minmax(0,1fr)] backdrop:bg-[var(--color-backdrop)] backdrop:backdrop-blur-sm sm:w-[min(1120px,calc(100%_-_2rem))] max-[560px]:m-0 max-[560px]:h-dvh max-[560px]:max-h-dvh max-[560px]:w-screen max-[560px]:rounded-none max-[560px]:border-0"
      onClose={onClose}
      onCancel={(event) => {
        if (saving) event.preventDefault()
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
            Fill in the test definition or import an existing Markdown file. We
            generate the Markdown and save it outside this repository without
            starting an execution.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            className="inline-flex min-h-10 items-center justify-center gap-2 rounded-lg border border-[var(--color-rule)] bg-panel-raised px-3 text-xs font-semibold text-[var(--color-ink-faint)] transition-colors hover:border-[var(--color-edge)] hover:text-ink"
            type="button"
            onClick={() => fileRef.current?.click()}
            disabled={busy}
          >
            <FileUp size={15} aria-hidden="true" />
            <span className="max-[460px]:sr-only">Import .md</span>
          </button>
          <button
            className="grid h-10 w-10 place-items-center rounded-lg border border-[var(--color-rule)] bg-transparent text-[var(--color-ink-faint)] transition-colors hover:border-[var(--color-edge)] hover:text-ink"
            type="button"
            onClick={onClose}
            disabled={saving}
            aria-label="Close local test creation"
          >
            <X size={16} aria-hidden="true" />
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
                className="m-0 flex items-center gap-2 rounded-lg bg-[var(--surface-soft)] px-3 py-2 text-xs text-[var(--color-ink-faint)]"
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
                  These fields become the title, version and local file
                  identity.
                </p>
              </div>
              <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                Test name
                <input
                  className={fieldClassName()}
                  value={draft.title}
                  onChange={(event) => updateDraft('title', event.target.value)}
                  disabled={busy}
                  autoFocus
                  required
                />
              </label>
              <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_8rem]">
                <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                  Markdown file name
                  <input
                    className={`${fieldClassName()} font-mono text-xs`}
                    value={fileName}
                    onChange={(event) => {
                      setFileName(event.target.value)
                      setError(null)
                    }}
                    disabled={busy}
                    spellCheck={false}
                    aria-invalid={!safeFileName}
                    aria-describedby="local-file-name-help"
                    required
                  />
                  <small
                    className={`font-normal leading-4 ${safeFileName ? 'text-ink-muted' : 'text-danger'}`}
                    id="local-file-name-help"
                  >
                    {safeFileName
                      ? `Compiles to ${compiledId(fileName.trim())}`
                      : 'Use letters, numbers, spaces, hyphens or underscores and end with .md.'}
                  </small>
                </label>
                <label className="grid content-start gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                  Version
                  <input
                    className={`${fieldClassName()} font-mono text-xs`}
                    type="number"
                    min="1"
                    step="1"
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
              <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                Before test
                <textarea
                  className={textareaClassName()}
                  value={draft.beforeTest}
                  onChange={(event) =>
                    updateDraft('beforeTest', event.target.value)
                  }
                  disabled={busy}
                  required
                />
              </label>
              <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                Task prompt
                <textarea
                  className={textareaClassName()}
                  value={draft.prompt}
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
                    Define the evidence and distribute exactly 100% across the
                    criteria.
                  </p>
                </div>
                <button
                  className="inline-flex min-h-9 items-center justify-center gap-2 rounded-lg border border-[var(--color-rule)] bg-panel-raised px-3 text-xs font-semibold text-[var(--color-ink-faint)] hover:border-[var(--color-edge)] hover:text-ink"
                  type="button"
                  onClick={addValidation}
                  disabled={busy}
                >
                  <Plus size={14} aria-hidden="true" />
                  Add criterion
                </button>
              </div>

              <div className="grid gap-3">
                {draft.validations.map((validation, index) => (
                  <fieldset
                    className="m-0 grid gap-3 rounded-lg border border-[var(--color-rule)] bg-panel-raised p-4"
                    key={validation.id}
                  >
                    <legend className="sr-only">Validation {index + 1}</legend>
                    <div className="flex items-center justify-between gap-3">
                      <strong className="font-mono text-xs font-semibold text-[var(--color-ink-faint)]">
                        Criterion {index + 1}
                      </strong>
                      <button
                        className="inline-flex min-h-8 items-center gap-1.5 rounded-lg px-2 text-xs font-semibold text-ink-muted hover:bg-[var(--surface-soft)] hover:text-danger disabled:opacity-40"
                        type="button"
                        onClick={() => removeValidation(validation.id)}
                        disabled={busy || draft.validations.length === 1}
                        aria-label={`Remove validation ${index + 1}`}
                      >
                        <Trash2 size={13} aria-hidden="true" />
                        Remove
                      </button>
                    </div>
                    <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem]">
                      <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                        Name
                        <input
                          className={fieldClassName()}
                          value={validation.title}
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
                      <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                        Weight %
                        <input
                          className={`${fieldClassName()} font-mono text-xs`}
                          type="number"
                          min="1"
                          max="100"
                          step="1"
                          value={validation.weight}
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
                    <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
                      Evaluation instructions
                      <textarea
                        className={`${textareaClassName()} min-h-24`}
                        value={validation.instructions}
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
                ))}
              </div>
            </section>
          </div>

          <aside className="grid min-w-0 content-start gap-5 bg-panel-raised p-5 sm:p-6 lg:sticky lg:top-0 lg:col-span-4">
            <div className="grid gap-2">
              <span className="text-xs font-semibold text-[var(--color-ink-faint)]">
                Generated Markdown
              </span>
              <code className="break-all rounded-lg border border-[var(--color-rule)] bg-panel px-3 py-2 font-mono text-xs text-[var(--color-accent)]">
                {fileName.trim() || 'local-scenario.md'}
              </code>
              <p className="m-0 text-xs leading-5 text-ink-muted">
                Plan <code>local</code> · {draft.validations.length} validation
                {draft.validations.length === 1 ? '' : 's'} · no execution
              </p>
            </div>

            <div className="grid gap-2 border-y border-[var(--color-rule)] py-4">
              <span className="text-xs font-semibold text-[var(--color-ink-faint)]">
                Validation weight
              </span>
              <output
                className={`font-mono text-2xl font-semibold tracking-[-0.04em] ${weight === 100 ? 'text-[var(--color-ok)]' : 'text-warning'}`}
                aria-live="polite"
              >
                {weight}%
              </output>
              <small className="text-xs leading-4 text-ink-muted">
                The generated criteria must total exactly 100%.
              </small>
            </div>

            <details className="group rounded-lg border border-[var(--color-rule)] bg-panel p-3">
              <summary className="cursor-pointer text-xs font-semibold text-[var(--color-ink-faint)]">
                Preview Markdown
              </summary>
              <pre className="mt-3 mb-0 max-h-72 overflow-auto whitespace-pre-wrap break-words border-t border-[var(--color-rule)] pt-3 font-mono text-[0.68rem] leading-5 text-ink-muted">
                {source}
              </pre>
            </details>

            {error ? (
              <p
                className="m-0 rounded-lg border border-[color-mix(in_srgb,var(--color-alert)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-alert)_8%,var(--surface))] p-3 text-xs leading-5 text-danger"
                role="alert"
              >
                {error}
              </p>
            ) : draftIssue || !safeFileName ? (
              <p className="m-0 text-xs leading-5 text-ink-muted">
                {!safeFileName
                  ? 'Enter a safe Markdown file name to continue.'
                  : draftIssue}
              </p>
            ) : (
              <p className="m-0 text-xs leading-5 text-[var(--color-ok)]">
                The definition is ready to be saved locally.
              </p>
            )}

            <button
              className="inline-flex min-h-11 w-full items-center justify-center gap-2 rounded-lg border border-[var(--color-accent)] bg-[var(--color-accent)] px-4 text-sm font-semibold text-[var(--color-accent-fg)] hover:bg-[color-mix(in_srgb,var(--color-accent)_88%,white)] disabled:cursor-not-allowed disabled:opacity-45"
              type="submit"
              form="local-scenario-form"
              disabled={!canCreate}
              aria-busy={saving}
            >
              <Plus size={15} aria-hidden="true" />
              {saving ? 'Creating test…' : 'Create test'}
            </button>
          </aside>
        </form>
      </div>
    </dialog>
  )
}
