import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
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
      className="local-runner-dialog"
      onClose={onClose}
      onKeyDownCapture={trapDialogFocus}
      aria-labelledby="local-runner-title"
    >
      <div className="local-runner-dialog-header">
        <div>
          <div className="section-kicker">New evidence</div>
          <strong id="local-runner-title">Configure execution</strong>
        </div>
        <button
          className="dialog-close"
          type="button"
          onClick={onClose}
          aria-label="Close execution form"
        >
          ×
        </button>
      </div>
      <section
        className="panel local-runner"
        aria-labelledby="local-runner-heading"
      >
        <div className="panel-heading local-runner-heading">
          <div>
            <div className="section-kicker">Local experiment</div>
            <h2 id="local-runner-heading">Run E2E scenarios</h2>
            <p className="trend-description">
              Use the Harness running at the selected iii URL. Each run is saved
              as an independent execution.
            </p>
          </div>
          <span className="local-run-status">{statusLabel(job?.status)}</span>
        </div>
        <div className="local-connection" aria-live="polite">
          <span
            className={`local-connection-dot ${catalog ? 'connected' : ''}`}
            aria-hidden="true"
          />
          <span>
            {catalog
              ? `${catalog.models.length} registered model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} scenarios`
              : 'Catalog loads when this dialog opens'}
          </span>
          <code>{form.url}</code>
          <button
            className="button"
            type="button"
            onClick={() => void refreshCatalog()}
            disabled={loadingCatalog || active}
          >
            {loadingCatalog ? 'Refreshing…' : 'Refresh catalog'}
          </button>
        </div>
        <form
          id="local-runner-form"
          className="local-run-form"
          onSubmit={submit}
        >
          <div className="local-field">
            <span>
              Execution label <small>optional</small>
            </span>
            <input
              value={form.label}
              maxLength={120}
              placeholder="Before system prompt change"
              onChange={(event) => update('label', event.target.value)}
              disabled={active}
            />
          </div>
          <div className="local-field">
            <span>
              iii WebSocket URL <small>required</small>
            </span>
            <input
              value={form.url}
              required
              placeholder="ws://127.0.0.1:49134"
              onChange={(event) => update('url', event.target.value)}
              disabled={active}
            />
          </div>
          <div className="local-field">
            <span>
              Execution model <small>required</small>
            </span>
            <ProviderModelDropdown
              ariaLabel="Execution model"
              required
              value={form.subject}
              onChange={(value) => update('subject', value)}
              disabled={active || !catalog}
              groups={groupedModels.map((group) => ({
                provider: group.provider,
                models: group.models.map((model) => ({
                  label: model.model,
                  value: modelKey(model),
                })),
              }))}
              placeholder="Choose a model"
            />
          </div>
          <div className="local-field">
            <span>
              Judge model <small>automatic when blank</small>
            </span>
            <ProviderModelDropdown
              ariaLabel="Judge model"
              value={form.judge}
              onChange={(value) => update('judge', value)}
              disabled={active || !catalog}
              groups={groupedModels.map((group) => ({
                provider: group.provider,
                models: group.models.map((model) => ({
                  label: model.model,
                  value: modelKey(model),
                })),
              }))}
              placeholder="Use execution model when required"
            />
          </div>
          <fieldset className="local-field local-field-wide">
            <legend>
              Scenarios <small>select one or more</small>
            </legend>
            <div className="local-scenario-toolbar">
              <button
                type="button"
                onClick={() => update('scenarios', catalog?.scenarios ?? [])}
                disabled={active || !catalog}
              >
                Select all
              </button>
              <button
                type="button"
                onClick={() => update('scenarios', [])}
                disabled={active || !catalog}
              >
                Clear
              </button>
              <span>{form.scenarios.length} selected</span>
            </div>
            <div className="local-scenario-options">
              {(catalog?.scenarios ?? []).map((scenario) => (
                <label className="local-scenario-option" key={scenario}>
                  <input
                    type="checkbox"
                    checked={form.scenarios.includes(scenario)}
                    disabled={active}
                    onChange={(event) =>
                      update(
                        'scenarios',
                        event.target.checked
                          ? [...form.scenarios, scenario]
                          : form.scenarios.filter((item) => item !== scenario),
                      )
                    }
                  />
                  <span>{scenario.replaceAll('_', ' ')}</span>
                </label>
              ))}
            </div>
          </fieldset>
          <details className="local-advanced local-field-wide">
            <summary>Advanced options</summary>
            <div className="local-advanced-grid">
              <label className="local-field">
                <span>Runs</span>
                <input
                  type="number"
                  min="1"
                  max="20"
                  value={form.runs}
                  onChange={(event) => update('runs', event.target.value)}
                  disabled={active}
                />
              </label>
              <label className="local-field">
                <span>Technical retries</span>
                <input
                  type="number"
                  min="0"
                  max="3"
                  value={form.technicalRetries}
                  onChange={(event) =>
                    update('technicalRetries', event.target.value)
                  }
                  disabled={active}
                />
              </label>
              <label className="local-field">
                <span>
                  Case seed <small>canonical when blank</small>
                </span>
                <input
                  type="number"
                  min="0"
                  step="1"
                  value={form.seed}
                  onChange={(event) => update('seed', event.target.value)}
                  disabled={active}
                />
              </label>
            </div>
          </details>
        </form>
        {error && (
          <p className="local-run-error" role="alert">
            {error}
          </p>
        )}
        {log && (
          <details className="local-run-log-shell" open={active}>
            <summary>Live runner output</summary>
            <pre className="local-run-log" aria-live="polite">
              {log}
            </pre>
          </details>
        )}
      </section>
      <div className="local-run-actions">
        <button
          className="button local-run-submit min-w-[190px] justify-center"
          type="submit"
          form="local-runner-form"
          disabled={
            active ||
            !catalog ||
            !selectedSubject ||
            form.scenarios.length === 0
          }
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
            className="button"
            type="button"
            onClick={() => void cancel()}
          >
            Cancel
          </button>
        )}
      </div>
    </dialog>
  )
}
