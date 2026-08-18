import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ExecutionSetup,
  ExecutionSetupReview,
} from '@/components/ExecutionSetup'
import { hashForNewPlan } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  JsonObject,
} from '@/lib/dashboard-data-source'

type RunnerModel = { provider: string; model: string }
type RunnerCatalog = { url: string; models: RunnerModel[]; scenarios: string[] }
type RunnerJob = {
  id?: string
  status?: string
  log?: string
  log_offset?: number
  log_truncated?: boolean
  error?: string | null
  defaults?: JsonObject
}

type RunnerForm = {
  label: string
  url: string
  subject: string
  judge: string
  scenarios: string[]
  runs: string
  technicalRetries: string
  seed: string
}

const initialForm: RunnerForm = {
  label: '',
  url: '',
  subject: '',
  judge: '',
  scenarios: [],
  runs: '1',
  technicalRetries: '1',
  seed: '',
}

function modelKey(model: RunnerModel) {
  return `${model.provider}\n${model.model}`
}

function modelGroups(models: RunnerModel[]) {
  const groups = new Map<string, RunnerModel[]>()
  for (const model of models) {
    const entries = groups.get(model.provider) ?? []
    if (!entries.some((entry) => entry.model === model.model))
      entries.push(model)
    groups.set(model.provider, entries)
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([provider, entries]) => ({
      provider,
      models: entries.sort((left, right) =>
        left.model.localeCompare(right.model),
      ),
    }))
}

function asCatalog(value: JsonObject): RunnerCatalog {
  const models = Array.isArray(value.models)
    ? value.models.flatMap((candidate) => {
        if (!candidate || typeof candidate !== 'object') return []
        const model = candidate as JsonObject
        if (
          typeof model.model !== 'string' ||
          typeof model.provider !== 'string'
        ) {
          return []
        }
        return [{ model: model.model, provider: model.provider }]
      })
    : []
  const scenarios = Array.isArray(value.scenarios)
    ? value.scenarios.filter(
        (scenario): scenario is string => typeof scenario === 'string',
      )
    : []
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios,
  }
}

function asJob(value: JsonObject): RunnerJob {
  const job = value.job && typeof value.job === 'object' ? value.job : value
  return job && typeof job === 'object' ? (job as RunnerJob) : {}
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function trapDialogFocus(event: React.KeyboardEvent<HTMLDialogElement>) {
  if (event.key !== 'Tab') return
  const focusable = [
    ...event.currentTarget.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])',
    ),
  ].filter((element) => !element.hidden && element.getClientRects().length > 0)
  if (focusable.length === 0) return

  const activeIndex = focusable.indexOf(document.activeElement as HTMLElement)
  const next = event.shiftKey
    ? activeIndex <= 0
      ? focusable.at(-1)
      : undefined
    : activeIndex === -1 || activeIndex === focusable.length - 1
      ? focusable[0]
      : undefined
  if (!next) return
  event.preventDefault()
  next.focus()
}

function statusLabel(status: string | undefined) {
  return (
    {
      running: 'Running…',
      cancelling: 'Cancelling…',
      cancelled: 'Cancelled',
      completed: 'Results saved',
      failed: 'Runner failed',
    }[status ?? ''] ?? 'Ready'
  )
}

export function LocalRunnerDialog({
  bridge,
  open,
  onClose,
  onCompleted,
}: {
  bridge: DashboardDataBridge | null
  open: boolean
  onClose: () => void
  onCompleted?: () => void
}) {
  const dialogRef = useRef<HTMLDialogElement>(null)
  const [catalog, setCatalog] = useState<RunnerCatalog | null>(null)
  const [form, setForm] = useState<RunnerForm>(initialForm)
  const [scenarioQuery, setScenarioQuery] = useState('')
  const [job, setJob] = useState<RunnerJob | null>(null)
  const [log, setLog] = useState('')
  const [logOffset, setLogOffset] = useState(0)
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refreshJob = useCallback(async () => {
    if (bridge?.mode !== 'local') return
    try {
      const response = asJob(await bridge.getRunSnapshot(logOffset))
      setJob(response)
      if (response.log_truncated) setLog('[Earlier runner output omitted]\n')
      if (response.log) setLog((current) => `${current}${response.log}`)
      if (typeof response.log_offset === 'number')
        setLogOffset(response.log_offset)
      if (response.status === 'completed') onCompleted?.()
    } catch (cause) {
      setError(errorMessage(cause))
    }
  }, [bridge, logOffset, onCompleted])

  const refreshCatalog = useCallback(async () => {
    if (bridge?.mode !== 'local') return
    setLoadingCatalog(true)
    setError(null)
    try {
      const next = asCatalog(await bridge.getCatalog(form.url || undefined))
      setCatalog(next)
      setForm((current) => ({
        ...current,
        url: current.url || next.url,
        subject:
          current.subject || (next.models[0] ? modelKey(next.models[0]) : ''),
        // Keep the local loop deliberate: selecting every scenario is too
        // expensive for the default path. The developer chooses the scope.
        scenarios: current.scenarios,
      }))
    } catch (cause) {
      setCatalog(null)
      setError(errorMessage(cause))
    } finally {
      setLoadingCatalog(false)
    }
  }, [bridge, form.url])

  useEffect(() => {
    const dialog = dialogRef.current
    if (!dialog) return
    if (open && !dialog.open) dialog.showModal()
    if (!open && dialog.open) dialog.close()
  }, [open])

  useEffect(() => {
    if (!open || !bridge || bridge.mode !== 'local') return
    setError(null)
    void refreshCatalog()
    void refreshJob()
    let unsubscribe: (() => void) | undefined
    let interval: number | undefined
    bridge
      .subscribeRunChanges(() => void refreshJob())
      .then((off) => {
        unsubscribe = off
      })
      .catch(() => undefined)
    interval = window.setInterval(() => void refreshJob(), 2_000)
    return () => {
      unsubscribe?.()
      if (interval) window.clearInterval(interval)
    }
  }, [bridge, open, refreshCatalog, refreshJob])

  const active =
    job?.status === 'running' || job?.status === 'cancelling' || submitting
  const selectedSubject = catalog?.models.find(
    (model) => modelKey(model) === form.subject,
  )
  const selectedJudge = catalog?.models.find(
    (model) => modelKey(model) === form.judge,
  )
  const groupedModels = useMemo(
    () => modelGroups(catalog?.models ?? []),
    [catalog],
  )
  const modelOptions = groupedModels.map((group) => ({
    provider: group.provider,
    models: group.models.map((model) => ({
      label: model.model,
      value: modelKey(model),
    })),
  }))
  const runsPerScenario = Math.max(1, Number(form.runs) || 1)
  const technicalRetries = Math.max(0, Number(form.technicalRetries) || 0)
  const plannedRuns = form.scenarios.length * runsPerScenario
  const canRun = Boolean(
    catalog && selectedSubject && form.scenarios.length > 0,
  )

  const update = <K extends keyof RunnerForm>(key: K, value: RunnerForm[K]) => {
    setForm((current) => ({ ...current, [key]: value }))
  }

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!bridge || !selectedSubject || form.scenarios.length === 0) {
      setError('Choose an execution model and at least one scenario.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const response = await bridge.startRun({
        // RunRequest.label is intentionally a string: empty labels remain
        // compatible with persisted execution metadata.
        label: form.label.trim(),
        url: form.url,
        model: selectedSubject.model,
        provider: selectedSubject.provider,
        judge_model: selectedJudge?.model || '',
        judge_provider: selectedJudge?.provider || '',
        scenarios: form.scenarios,
        runs: Number(form.runs),
        technical_retries: Number(form.technicalRetries),
        seed: form.seed ? Number(form.seed) : null,
      })
      setJob(asJob(response))
      setLog('')
      setLogOffset(0)
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setSubmitting(false)
    }
  }

  const cancel = async () => {
    if (!bridge) return
    try {
      setJob(asJob(await bridge.cancelRun()))
    } catch (cause) {
      setError(errorMessage(cause))
    }
  }

  return (
    <dialog
      ref={dialogRef}
      className="ds-root m-auto hidden max-h-[94dvh] w-[min(1180px,calc(100%_-_1rem))] max-w-none overflow-hidden rounded-xl border border-[var(--color-edge)] bg-[var(--color-panel)] p-0 text-[var(--color-ink)] shadow-[var(--shadow-panel)] open:grid open:grid-rows-[auto_minmax(0,1fr)] backdrop:bg-[var(--color-backdrop)] backdrop:backdrop-blur-sm sm:w-[min(1180px,calc(100%_-_2rem))]"
      onClose={onClose}
      onKeyDownCapture={trapDialogFocus}
      aria-labelledby="local-runner-title"
    >
      <header className="flex items-start justify-between gap-5 border-b border-[var(--color-rule)] bg-[var(--color-panel)] px-5 py-4 sm:px-6">
        <div className="min-w-0">
          <p className="m-0 font-mono text-[0.65rem] font-semibold uppercase tracking-[0.07em] text-[var(--color-accent)]">
            Execution setup
          </p>
          <strong
            className="mt-1 block text-base font-semibold"
            id="local-runner-title"
          >
            Create a quick benchmark result
          </strong>
          <p className="mt-1 mb-0 max-w-2xl text-xs leading-5 text-[var(--color-ink-ghost)]">
            Run selected scenarios once. Use a reusable plan when you need a
            fixed baseline and candidate comparison.
          </p>
        </div>
        <button
          className="grid h-11 w-11 shrink-0 place-items-center rounded-lg border border-[var(--color-rule)] bg-transparent text-lg text-[var(--color-ink-faint)] transition-colors hover:border-[var(--color-edge)] hover:text-[var(--color-ink)]"
          type="button"
          onClick={onClose}
          aria-label="Close execution form"
        >
          ×
        </button>
      </header>

      <div className="min-h-0 overflow-auto">
        <form
          id="local-runner-form"
          className="grid min-w-0 gap-px bg-[var(--color-rule)] lg:grid-cols-12"
          onSubmit={submit}
        >
          <div className="min-w-0 bg-[var(--color-panel)] lg:col-span-8">
            <ExecutionSetup
              idPrefix="quick-execution"
              mode="quick"
              label={form.label}
              url={form.url}
              subject={form.subject}
              judge={form.judge}
              modelGroups={modelOptions}
              availableScenarios={catalog?.scenarios ?? []}
              selectedScenarios={form.scenarios}
              query={scenarioQuery}
              runs={form.runs}
              technicalRetries={form.technicalRetries}
              seed={form.seed}
              disabled={active}
              catalogLoading={loadingCatalog}
              catalogSummary={
                catalog
                  ? `${catalog.models.length} registered model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} scenarios`
                  : 'Catalog loads when this dialog opens'
              }
              onRefreshCatalog={() => void refreshCatalog()}
              onLabelChange={(value) => update('label', value)}
              onUrlChange={(value) => update('url', value)}
              onSubjectChange={(value) => update('subject', value)}
              onJudgeChange={(value) => update('judge', value)}
              onSelectedScenariosChange={(value) => update('scenarios', value)}
              onQueryChange={setScenarioQuery}
              onRunsChange={(value) => update('runs', value)}
              onTechnicalRetriesChange={(value) =>
                update('technicalRetries', value)
              }
              onSeedChange={(value) => update('seed', value)}
            />

            {error && (
              <p
                className="m-0 border-t border-[var(--color-rule)] bg-[color-mix(in_srgb,var(--color-alert)_8%,var(--color-panel))] p-5 text-xs leading-5 text-[var(--color-alert)] sm:px-6"
                role="alert"
              >
                {error}
              </p>
            )}
            {log && (
              <details
                className="border-t border-[var(--color-rule)] p-5 sm:p-6"
                open={active}
              >
                <summary className="cursor-pointer text-xs font-semibold text-[var(--color-ink-faint)]">
                  Live runner output
                </summary>
                <pre
                  className="mt-3 mb-0 max-h-80 w-full overflow-auto rounded-lg border border-[var(--color-rule)] bg-[var(--color-bg)] p-4 font-mono text-[0.68rem] leading-5 text-[var(--color-ink-faint)]"
                  aria-live="polite"
                >
                  {log}
                </pre>
              </details>
            )}
          </div>

          <div className="min-w-0 bg-[var(--color-panel-raised)] lg:col-span-4">
            <ExecutionSetupReview
              mode="quick"
              status={
                job?.status
                  ? statusLabel(job.status)
                  : canRun
                    ? 'Ready'
                    : 'Incomplete'
              }
              subject={
                selectedSubject
                  ? `${selectedSubject.provider} / ${selectedSubject.model}`
                  : ''
              }
              judge={
                selectedJudge
                  ? `${selectedJudge.provider} / ${selectedJudge.model}`
                  : ''
              }
              url={form.url}
              selectedScenarios={form.scenarios.length}
              plannedRuns={plannedRuns}
              runsPerScenario={runsPerScenario}
              technicalRetries={technicalRetries}
              ready={canRun && !active}
            >
              <button
                className="inline-flex min-h-11 w-full items-center justify-center rounded-lg border border-[var(--color-accent)] bg-[var(--color-accent)] px-4 text-sm font-semibold text-[var(--color-accent-fg)] transition-colors hover:bg-[color-mix(in_srgb,var(--color-accent)_88%,white)] disabled:cursor-not-allowed disabled:opacity-45"
                type="submit"
                form="local-runner-form"
                disabled={active || !canRun}
                aria-busy={active}
              >
                {submitting
                  ? 'Starting E2E…'
                  : active
                    ? 'E2E running…'
                    : 'Run selected E2E'}
              </button>
              {active && (
                <button
                  className="inline-flex min-h-11 w-full items-center justify-center rounded-lg border border-[var(--color-edge)] bg-transparent px-4 text-sm font-semibold text-[var(--color-ink-faint)] hover:text-[var(--color-ink)]"
                  type="button"
                  onClick={() => void cancel()}
                >
                  Cancel execution
                </button>
              )}
              <a
                className="inline-flex min-h-10 w-full items-center justify-center rounded-lg px-3 text-xs font-semibold text-[var(--color-ink-faint)] underline-offset-4 hover:text-[var(--color-ink)] hover:underline"
                href={hashForNewPlan()}
              >
                Create a reusable plan instead
              </a>
            </ExecutionSetupReview>
          </div>
        </form>
      </div>
    </dialog>
  )
}
