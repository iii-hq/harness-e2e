import { Copy, Download, FileUp, Plus, X } from 'lucide-react'
import { useMemo, useRef, useState } from 'react'
import { DiscardDraftDialog } from '@/components/DiscardDraftDialog'
import {
  buttonClassName,
  Dialog,
  Field,
  fieldDescribedBy,
  Input,
  Textarea,
} from '@/design-system'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'
import {
  buildLocalScenarioSource,
  distributeValidationWeights,
  INITIAL_LOCAL_SCENARIO_DRAFT,
  LOCAL_SCENARIO_TEMPLATE,
  type LocalScenarioDraft,
  type LocalScenarioFieldIssues,
  localScenarioDraftFieldIssues,
  localScenarioValidationWeight,
  parseLocalScenarioSource,
} from '@/lib/local-scenario-authoring'

export { LOCAL_SCENARIO_TEMPLATE }

const MAX_LOCAL_SCENARIO_BYTES = 256 * 1024

export type LocalScenarioCreatedIntent = 'catalog' | 'plan'

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

/** Human label for an issue key, used in the footer's pending list. */
export function issueFieldLabel(key: string, draft: LocalScenarioDraft) {
  const names: Record<string, string> = {
    title: 'test name',
    file: 'file name',
    version: 'version',
    beforeTest: 'before test',
    prompt: 'task prompt',
    validations: 'criteria weights',
  }
  if (names[key]) return names[key]
  const match = key.match(/^validation:(.+):(title|weight|instructions)$/)
  if (!match) return key
  const index = draft.validations.findIndex(
    (validation) => validation.id === match[1],
  )
  const part =
    match[2] === 'title'
      ? 'name'
      : match[2] === 'weight'
        ? 'weight'
        : 'instructions'
  return `criterion ${index + 1} ${part}`
}

function fieldId(key: string) {
  return `local-test-${key.replace(/[^a-z0-9]+/gi, '-')}`
}

function responseScenarioId(value: Record<string, unknown>) {
  const scenario = value.scenario
  if (!scenario || typeof scenario !== 'object') return null
  const id = (scenario as Record<string, unknown>).scenario_id
  return typeof id === 'string' ? id : null
}

const TEMPLATE_HREF = `data:text/markdown;charset=utf-8,${encodeURIComponent(LOCAL_SCENARIO_TEMPLATE)}`

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
  onCreated: (scenarioId: string, intent: LocalScenarioCreatedIntent) => void
}) {
  const fileRef = useRef<HTMLInputElement>(null)
  const nextValidationId = useRef(2)
  // null means "derived from the test name"; a string is a manual edit.
  const [fileNameInput, setFileNameInput] = useState<string | null>(null)
  const [draft, setDraft] = useState<LocalScenarioDraft>(cloneEmptyDraft)
  const [saving, setSaving] = useState(false)
  const [attempted, setAttempted] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [confirmingClose, setConfirmingClose] = useState(false)

  const fileName =
    fileNameInput ?? (fileNameForTitle(draft.title) || initialFileName)
  const source = useMemo(() => buildLocalScenarioSource(draft), [draft])
  const weight = localScenarioValidationWeight(draft)
  const safeFileName = safeLocalFileName(fileName.trim())
  const allIssues = useMemo<LocalScenarioFieldIssues>(() => {
    const issues = localScenarioDraftFieldIssues(draft)
    if (!safeFileName)
      issues.file =
        'Use letters, numbers, spaces, hyphens or underscores and end with .md.'
    return issues
  }, [draft, safeFileName])
  // Audit NT-02 / NT-05: the errors appear after the first attempt, on the
  // field itself, and the primary stays enabled.
  const issues = attempted ? allIssues : {}
  const issueKeys = Object.keys(issues)
  const busy = disabled || saving
  const dirty = isLocalScenarioDraftDirty(draft, fileNameInput !== null)
  const compiledId = compiledLocalScenarioId(fileName.trim())

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

  const focusIssue = (key: string) => {
    const element = document.getElementById(fieldId(key))
    if (element instanceof HTMLElement) element.focus()
  }

  const save = async (intent: LocalScenarioCreatedIntent) => {
    const keys = Object.keys(allIssues)
    if (keys.length > 0) {
      setAttempted(true)
      focusIssue(keys[0])
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
      onCreated(scenarioId, intent)
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
      // Audit NT-10: the parse error names the problem, in the one feedback area.
      setError(
        `Could not import ${file.name}: ${cause instanceof Error ? cause.message : String(cause)}`,
      )
    } finally {
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  const copySource = () => {
    void navigator.clipboard?.writeText(source).then(() => {
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    })
  }

  const describedBy = (key: string, hint = false) =>
    fieldDescribedBy(fieldId(key), { hint, error: Boolean(issues[key]) })

  const preview = (
    <>
      <div className="flex items-center justify-between gap-2">
        <span className="ds-label">generated markdown · live</span>
        <button
          className={buttonClassName({ variant: 'quiet', size: 'compact' })}
          type="button"
          onClick={copySource}
          aria-label="Copy the generated Markdown"
        >
          <Copy size={13} aria-hidden="true" />
          {copied ? 'copied' : 'copy'}
        </button>
      </div>
      <pre
        className="m-0 max-h-[60vh] overflow-auto whitespace-pre-wrap break-words rounded-[6px] bg-canvas p-3 font-mono text-xs leading-5 text-ink-soft"
        data-markdown-preview
      >
        {source}
      </pre>
      <p className="m-0 font-mono text-label text-ink-muted">
        compiles to test id{' '}
        <span className="text-ink-soft" data-compiled-id>
          {compiledId}
        </span>{' '}
        · saved outside this repository · no execution
      </p>
    </>
  )

  // Audit NT-01 / NT-12: the state line and the actions stay visible at
  // every width; the pending list links to each field.
  const footer = (
    <>
      <div
        className="min-w-0 flex-1 text-xs leading-5"
        role={error ? 'alert' : 'status'}
        aria-live="polite"
      >
        {error ? (
          <span className="text-danger">{error}</span>
        ) : issueKeys.length > 0 ? (
          <span className="flex flex-wrap items-center gap-x-1 text-ink-soft">
            {issueKeys.length} field{issueKeys.length === 1 ? '' : 's'} need
            {issueKeys.length === 1 ? 's' : ''} attention ·{' '}
            {issueKeys.map((key, index) => (
              <button
                key={key}
                className="rounded-[6px] border-0 bg-transparent p-0 font-mono text-label text-danger underline-offset-2 hover:underline"
                type="button"
                onClick={() => focusIssue(key)}
              >
                {issueFieldLabel(key, draft)}
                {index < issueKeys.length - 1 ? ',' : ''}
              </button>
            ))}
          </span>
        ) : notice ? (
          <span className="text-ink-soft">{notice}</span>
        ) : (
          <span className="text-ink-soft">
            Saved locally as Markdown. No execution starts.
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button
          className={buttonClassName({ variant: 'secondary' })}
          type="button"
          onClick={requestClose}
          disabled={saving}
        >
          cancel
        </button>
        <button
          className={buttonClassName({ variant: 'secondary' })}
          type="button"
          onClick={() => void save('plan')}
          disabled={busy}
        >
          create and open plan
        </button>
        <button
          className={buttonClassName({ variant: 'primary' })}
          type="submit"
          form="local-scenario-form"
          disabled={busy}
          aria-busy={saving}
        >
          <Plus size={15} aria-hidden="true" />
          {saving ? 'creating test…' : 'create test'}
        </button>
      </div>
    </>
  )

  return (
    <Dialog
      open
      onClose={requestClose}
      size="xl"
      tall
      kicker="markdown"
      title="new local test"
      description="Describe the task, then how to judge it. The file is saved outside this repository; nothing runs."
      closeLabel="Close local test creation"
      className="ds-root"
      actions={
        <>
          <a
            className={buttonClassName({
              variant: 'quiet',
              size: 'compact',
              className: 'no-underline',
            })}
            href={TEMPLATE_HREF}
            download="new-test.md"
            title="Download the template .md"
            aria-label="Download the template .md"
          >
            <Download size={14} aria-hidden="true" />
            <span className="max-[560px]:sr-only">template .md</span>
          </a>
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            type="button"
            onClick={() => fileRef.current?.click()}
            disabled={busy}
            title="Import a Markdown test definition"
            aria-label="Import a Markdown test definition"
          >
            <FileUp size={14} aria-hidden="true" />
            <span className="max-[560px]:sr-only">import .md</span>
          </button>
        </>
      }
      footer={footer}
    >
      <form
        id="local-scenario-form"
        className="grid min-w-0 gap-6 @[1024px]:grid-cols-[minmax(0,1fr)_22rem] @[1024px]:items-start"
        onSubmit={(event) => {
          event.preventDefault()
          void save('catalog')
        }}
        noValidate
      >
        <input
          ref={fileRef}
          className="hidden"
          type="file"
          accept=".md,text/markdown,text/plain"
          onChange={(event) => void importFile(event.target.files?.[0])}
          disabled={busy}
        />

        <div className="grid min-w-0 content-start gap-7">
          <section className="grid gap-4" aria-labelledby="test-details-title">
            <h3
              className="m-0 text-sm font-semibold text-ink"
              id="test-details-title"
            >
              Test details
            </h3>
            <Field
              label="Test name"
              htmlFor={fieldId('title')}
              meta="required"
              error={issues.title}
            >
              <Input
                id={fieldId('title')}
                value={draft.title}
                placeholder="Database recovery"
                onChange={(event) => updateDraft('title', event.target.value)}
                disabled={busy}
                autoFocus
                aria-invalid={issues.title ? true : undefined}
                aria-describedby={describedBy('title')}
              />
            </Field>
            <div className="grid items-start gap-4 sm:grid-cols-[minmax(0,1fr)_7rem]">
              <Field
                label="File"
                htmlFor={fieldId('file')}
                meta={fileNameInput === null ? 'from name' : 'custom'}
                hint={`compiles to ${compiledId}`}
                error={issues.file}
              >
                <Input
                  id={fieldId('file')}
                  className="font-mono text-xs"
                  value={fileName}
                  onChange={(event) => {
                    setFileNameInput(event.target.value)
                    setError(null)
                  }}
                  disabled={busy}
                  spellCheck={false}
                  aria-invalid={issues.file ? true : undefined}
                  aria-describedby={describedBy('file', true)}
                />
              </Field>
              <Field
                label="Version"
                htmlFor={fieldId('version')}
                error={issues.version}
              >
                <Input
                  id={fieldId('version')}
                  className="font-mono text-xs"
                  type="number"
                  min="1"
                  step="1"
                  inputMode="numeric"
                  value={draft.version}
                  onChange={(event) =>
                    updateDraft('version', event.target.value)
                  }
                  disabled={busy}
                  aria-invalid={issues.version ? true : undefined}
                  aria-describedby={describedBy('version')}
                />
              </Field>
            </div>
          </section>

          <section
            className="grid gap-4"
            aria-labelledby="test-instructions-title"
          >
            <h3
              className="m-0 text-sm font-semibold text-ink"
              id="test-instructions-title"
            >
              Instructions
            </h3>
            <Field
              label="Before test"
              htmlFor={fieldId('beforeTest')}
              meta="setup the harness runs first"
              error={issues.beforeTest}
            >
              <Textarea
                id={fieldId('beforeTest')}
                className="min-h-24"
                value={draft.beforeTest}
                placeholder={PLACEHOLDER.beforeTest}
                onChange={(event) =>
                  updateDraft('beforeTest', event.target.value)
                }
                disabled={busy}
                aria-invalid={issues.beforeTest ? true : undefined}
                aria-describedby={describedBy('beforeTest')}
              />
            </Field>
            <Field
              label="Task prompt"
              htmlFor={fieldId('prompt')}
              meta="required"
              hint="Say what “done” looks like — the judge reads this prompt too."
              error={issues.prompt}
            >
              <Textarea
                id={fieldId('prompt')}
                className="min-h-28"
                value={draft.prompt}
                placeholder={PLACEHOLDER.prompt}
                onChange={(event) => updateDraft('prompt', event.target.value)}
                disabled={busy}
                aria-invalid={issues.prompt ? true : undefined}
                aria-describedby={describedBy('prompt', true)}
              />
            </Field>
          </section>

          <section
            className="grid gap-3"
            aria-labelledby="validation-criteria-title"
          >
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h3
                  className="m-0 text-sm font-semibold text-ink"
                  id="validation-criteria-title"
                >
                  Validation criteria
                </h3>
                <p className="mt-1 mb-0 text-xs leading-5 text-ink-soft">
                  Weights must total 100.
                </p>
              </div>
              <div className="flex flex-wrap gap-2">
                <button
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                  })}
                  type="button"
                  onClick={() =>
                    setDraft((current) => distributeValidationWeights(current))
                  }
                  disabled={busy || draft.validations.length === 0}
                >
                  distribute evenly
                </button>
                <button
                  className={buttonClassName({
                    variant: 'secondary',
                    size: 'compact',
                  })}
                  type="button"
                  onClick={addValidation}
                  disabled={busy}
                >
                  <Plus size={13} aria-hidden="true" />
                  add criterion
                </button>
              </div>
            </div>

            <div className="grid gap-3" data-criteria>
              {draft.validations.map((validation, index) => {
                const placeholder =
                  PLACEHOLDER.validations[index] ??
                  PLACEHOLDER.validations[PLACEHOLDER.validations.length - 1]
                const key = `validation:${validation.id}`
                const onlyOne = draft.validations.length === 1
                return (
                  // Audit NT-04 / NT-11: a compact fill block, no border.
                  <fieldset
                    className="m-0 grid gap-3 rounded-[6px] border-0 bg-[var(--surface-fill)] p-3"
                    key={validation.id}
                  >
                    <legend className="sr-only">Validation {index + 1}</legend>
                    <div className="grid items-start gap-3 sm:grid-cols-[minmax(0,1fr)_5.5rem_auto]">
                      <Field
                        label={`Criterion ${index + 1}`}
                        htmlFor={fieldId(`${key}:title`)}
                        error={issues[`${key}:title`]}
                      >
                        <Input
                          id={fieldId(`${key}:title`)}
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
                          aria-invalid={
                            issues[`${key}:title`] ? true : undefined
                          }
                          aria-describedby={describedBy(`${key}:title`)}
                        />
                      </Field>
                      <Field
                        label="Weight"
                        htmlFor={fieldId(`${key}:weight`)}
                        meta="%"
                        error={issues[`${key}:weight`]}
                      >
                        <Input
                          id={fieldId(`${key}:weight`)}
                          className="font-mono text-xs"
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
                          aria-invalid={
                            issues[`${key}:weight`] ? true : undefined
                          }
                          aria-describedby={describedBy(`${key}:weight`)}
                        />
                      </Field>
                      <button
                        className={buttonClassName({
                          variant: 'quiet',
                          size: 'compact',
                          className: 'sm:mt-6',
                        })}
                        type="button"
                        onClick={() => removeValidation(validation.id)}
                        disabled={busy || onlyOne}
                        title={
                          onlyOne
                            ? 'At least one criterion is required'
                            : `Remove criterion ${index + 1}`
                        }
                        aria-label={`Remove criterion ${index + 1}`}
                      >
                        <X size={13} aria-hidden="true" />
                      </button>
                    </div>
                    <Field
                      label="Evaluation instructions"
                      htmlFor={fieldId(`${key}:instructions`)}
                      error={issues[`${key}:instructions`]}
                    >
                      <Textarea
                        id={fieldId(`${key}:instructions`)}
                        className="min-h-20"
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
                        aria-invalid={
                          issues[`${key}:instructions`] ? true : undefined
                        }
                        aria-describedby={describedBy(`${key}:instructions`)}
                      />
                    </Field>
                  </fieldset>
                )
              })}
            </div>
            <output
              className={`font-mono text-xs tabular-nums ${weight === 100 ? 'text-success' : 'text-warning'}`}
              aria-label="Validation weight total"
              id={fieldId('validations')}
              tabIndex={-1}
            >
              {weightBreakdown(draft)}
              {weight === 100
                ? ' ✓'
                : weight < 100
                  ? ` · ${100 - weight} missing`
                  : ` · ${weight - 100} over`}
            </output>
            {issues.validations ? (
              <p className="m-0 text-xs text-danger" role="alert">
                {issues.validations}
              </p>
            ) : null}
          </section>
        </div>

        {/* Audit NT-07: the live Markdown is a side panel when there is room
            and a collapsed disclosure otherwise. */}
        <aside className="hidden min-w-0 content-start gap-3 @[1024px]:sticky @[1024px]:top-0 @[1024px]:grid">
          {preview}
        </aside>
        <details className="group min-w-0 rounded-[6px] bg-[var(--surface-fill)] p-3 @[1024px]:hidden">
          <summary className="cursor-pointer font-mono text-xs text-ink-soft">
            generated markdown
          </summary>
          <div className="mt-3 grid gap-3">{preview}</div>
        </details>
      </form>

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
    </Dialog>
  )
}
