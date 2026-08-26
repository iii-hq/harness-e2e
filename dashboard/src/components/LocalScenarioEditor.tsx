import { Check, FileUp, Plus, X } from 'lucide-react'
import { useMemo, useRef, useState } from 'react'
import type { DashboardDataBridge } from '@/lib/dashboard-data-source'

const REQUIRED_SECTIONS = [
  'Plans',
  'Version',
  'Before Test',
  'Prompt',
  'Validations',
] as const

export const LOCAL_SCENARIO_TEMPLATE = `# Local scenario

## Plans

- local

## Version

1

## Before Test

Prepare the isolated state required by this test. Keep every mutation run-scoped and reversible.

## Prompt

Describe the task the Harness must complete.

## Validations

### Expected outcome (70%)

Describe the evidence that proves the requested outcome is correct.

### Safe execution (30%)

Confirm the run stayed within the intended scope and left no residual state.
`

function compiledId(fileName: string) {
  const stem = fileName.replace(/\.md$/i, '')
  const normalized = stem
    .toLowerCase()
    .replace(/[- ]/g, '_')
    .replace(/[^a-z0-9_]/g, '')
  return normalized ? `markdown_${normalized}` : 'markdown_…'
}

function safeLocalFileName(fileName: string) {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9 _-]*\.md$/.test(fileName)) return false
  const id = compiledId(fileName).replace(/^markdown_/, '')
  return id !== '' && !id.includes('__')
}

function sectionCount(source: string, title: string) {
  const escaped = title.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  return source.match(new RegExp(`^## ${escaped}\\s*$`, 'gm'))?.length ?? 0
}

function validationWeight(source: string) {
  return [...source.matchAll(/^### .+ \((\d+)%\)\s*$/gm)].reduce(
    (total, match) => total + Number(match[1]),
    0,
  )
}

function responseScenarioId(value: Record<string, unknown>) {
  const scenario = value.scenario
  if (!scenario || typeof scenario !== 'object') return null
  const id = (scenario as Record<string, unknown>).scenario_id
  return typeof id === 'string' ? id : null
}

export function LocalScenarioEditor({
  bridge,
  disabled = false,
  onClose,
  onCreated,
}: {
  bridge: DashboardDataBridge
  disabled?: boolean
  onClose: () => void
  onCreated: (scenarioId: string) => void
}) {
  const fileRef = useRef<HTMLInputElement>(null)
  const [fileName, setFileName] = useState('local-scenario.md')
  const [source, setSource] = useState(LOCAL_SCENARIO_TEMPLATE)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const checks = useMemo(
    () =>
      REQUIRED_SECTIONS.map((section) => ({
        label: `## ${section}`,
        passed: sectionCount(source, section) === 1,
      })),
    [source],
  )
  const weight = useMemo(() => validationWeight(source), [source])
  const safeFileName = safeLocalFileName(fileName)

  const save = async () => {
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
    try {
      setFileName(file.name)
      setSource(await file.text())
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  return (
    <section
      className="border-b border-[var(--color-rule)] bg-panel"
      aria-labelledby="local-scenario-editor-title"
    >
      <header className="flex items-start justify-between gap-5 border-b border-[var(--color-rule)] px-5 py-4 sm:px-6">
        <div className="min-w-0 max-w-5xl">
          <h2
            className="m-0 text-base font-semibold tracking-[-0.02em] text-ink"
            id="local-scenario-editor-title"
          >
            Create a local test
          </h2>
          <p className="mt-1 mb-0 max-w-3xl text-xs leading-5 text-[var(--color-ink-ghost)]">
            Saved under the Harness E2E data directory, outside this repository.
            Creating a test does not start an execution or add it to a committed
            campaign.
          </p>
        </div>
        <button
          className="grid h-10 w-10 shrink-0 place-items-center rounded-lg border border-[var(--color-rule)] bg-transparent text-[var(--color-ink-faint)] transition-colors hover:border-[var(--color-edge)] hover:text-ink"
          type="button"
          onClick={onClose}
          disabled={saving}
          aria-label="Close local scenario editor"
        >
          <X size={16} aria-hidden="true" />
        </button>
      </header>

      <div className="grid grid-cols-1 gap-px bg-[var(--color-rule)] lg:grid-cols-12">
        <div className="grid min-w-0 gap-4 bg-panel p-5 sm:p-6 lg:col-span-8">
          <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
            <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
              Markdown file name
              <input
                className="min-h-11 rounded-lg border border-[var(--color-rule)] bg-panel-raised px-3 font-mono text-xs text-ink focus-visible:border-[var(--color-rule-focus)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-rule-focus)]"
                value={fileName}
                onChange={(event) => setFileName(event.target.value)}
                disabled={disabled || saving}
                spellCheck={false}
                aria-invalid={!safeFileName}
              />
            </label>
            <button
              className="inline-flex min-h-11 items-center justify-center gap-2 rounded-lg border border-[var(--color-rule)] bg-panel-raised px-4 text-xs font-semibold text-[var(--color-ink-faint)] hover:border-[var(--color-edge)] hover:text-ink"
              type="button"
              onClick={() => fileRef.current?.click()}
              disabled={disabled || saving}
            >
              <FileUp size={15} aria-hidden="true" />
              Import .md
            </button>
            <input
              ref={fileRef}
              className="sr-only"
              type="file"
              accept=".md,text/markdown,text/plain"
              onChange={(event) => void importFile(event.target.files?.[0])}
              tabIndex={-1}
            />
          </div>

          <label className="grid gap-2 text-xs font-semibold text-[var(--color-ink-faint)]">
            Scenario source
            <textarea
              className="min-h-[30rem] w-full resize-y rounded-lg border border-[var(--color-rule)] bg-[var(--color-bg)] p-4 font-mono text-xs leading-5 text-ink focus-visible:border-[var(--color-rule-focus)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-rule-focus)]"
              value={source}
              onChange={(event) => setSource(event.target.value)}
              disabled={disabled || saving}
              spellCheck={false}
            />
          </label>
        </div>

        <aside className="grid content-start gap-5 bg-panel-raised p-5 sm:p-6 lg:sticky lg:top-0 lg:col-span-4 lg:max-h-[42rem] lg:overflow-auto">
          <div className="grid gap-2">
            <span className="text-xs font-semibold text-[var(--color-ink-faint)]">
              Compiled identity
            </span>
            <code className="break-all rounded-lg border border-[var(--color-rule)] bg-panel px-3 py-2 font-mono text-xs text-[var(--color-accent)]">
              {compiledId(fileName)}
            </code>
            {!safeFileName && (
              <small className="text-xs leading-4 text-[var(--color-alert)]">
                Use letters, numbers, spaces, hyphens or underscores and end
                with .md.
              </small>
            )}
          </div>

          <div className="grid gap-2">
            <span className="text-xs font-semibold text-[var(--color-ink-faint)]">
              Required contract
            </span>
            <ul className="m-0 grid list-none gap-2 p-0">
              {checks.map((check) => (
                <li
                  className="flex items-center gap-2 font-mono text-[0.68rem] text-[var(--color-ink-ghost)]"
                  key={check.label}
                >
                  <span
                    className={`grid h-5 w-5 place-items-center rounded-full border ${
                      check.passed
                        ? 'border-[color-mix(in_srgb,var(--color-ok)_45%,transparent)] text-[var(--color-ok)]'
                        : 'border-[var(--color-edge)] text-[var(--color-ink-ghost)]'
                    }`}
                  >
                    {check.passed && <Check size={12} aria-hidden="true" />}
                  </span>
                  {check.label}
                </li>
              ))}
            </ul>
          </div>

          <div className="grid gap-2 border-y border-[var(--color-rule)] py-4">
            <span className="text-xs font-semibold text-[var(--color-ink-faint)]">
              Validation weight
            </span>
            <output
              className={`font-mono text-2xl font-semibold tracking-[-0.04em] ${
                weight === 100
                  ? 'text-[var(--color-ok)]'
                  : 'text-[var(--color-warn)]'
              }`}
            >
              {weight}%
            </output>
            <small className="text-xs leading-4 text-[var(--color-ink-ghost)]">
              Validation headings must use “### Name (N%)” and total exactly
              100%.
            </small>
          </div>

          {error && (
            <p
              className="m-0 rounded-lg border border-[color-mix(in_srgb,var(--color-alert)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-alert)_8%,var(--surface))] p-3 text-xs leading-5 text-[var(--color-alert)]"
              role="alert"
            >
              {error}
            </p>
          )}

          <button
            className="inline-flex min-h-11 w-full items-center justify-center gap-2 rounded-lg border border-[var(--color-accent)] bg-[var(--color-accent)] px-4 text-sm font-semibold text-[var(--color-accent-fg)] hover:bg-[color-mix(in_srgb,var(--color-accent)_88%,white)] disabled:cursor-not-allowed disabled:opacity-45"
            type="button"
            onClick={() => void save()}
            disabled={
              disabled || saving || !safeFileName || source.trim() === ''
            }
          >
            <Plus size={15} aria-hidden="true" />
            {saving ? 'Creating test…' : 'Create test'}
          </button>
        </aside>
      </div>
    </section>
  )
}
