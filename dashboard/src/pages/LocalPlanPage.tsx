import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DiscardDraftDialog } from '@/components/DiscardDraftDialog'
import {
  ExecutionSetup,
  ExecutionSetupReview,
  requestQuickExecution,
} from '@/components/ExecutionSetup'
import { buttonClassName } from '@/design-system'
import { useDirtyNavigation } from '@/hooks/use-dirty-navigation'
import {
  hashForPlan,
  hashForPlans,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type JsonObject,
} from '@/lib/dashboard-data-source'

type Model = { provider: string; model: string }
type Catalog = { url: string; models: Model[]; scenarios: string[] }

function modelKey(model: Model) {
  return [model.provider, model.model].join('\n')
}

function modelGroups(models: Model[]) {
  const groups = new Map<string, Model[]>()
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

function catalogValue(value: JsonObject): Catalog {
  const models = Array.isArray(value.models)
    ? value.models.flatMap((candidate) => {
        if (!candidate || typeof candidate !== 'object') return []
        const item = candidate as JsonObject
        return typeof item.provider === 'string' &&
          typeof item.model === 'string'
          ? [{ provider: item.provider, model: item.model }]
          : []
      })
    : []
  const localScenarioIds = new Set(
    Array.isArray(value.local_scenarios)
      ? value.local_scenarios.flatMap((candidate) => {
          if (!candidate || typeof candidate !== 'object') return []
          const id = (candidate as JsonObject).id
          return typeof id === 'string' ? [id] : []
        })
      : [],
  )
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios: Array.isArray(value.scenarios)
      ? value.scenarios.filter(
          (item): item is string =>
            typeof item === 'string' && !localScenarioIds.has(item),
        )
      : [],
  }
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** New-plan form defaults. One object feeds both the state and the dirty
 * baseline, so an untouched form can never start out dirty (audit PN-01). */
export const PLAN_FORM_DEFAULTS = {
  label: '',
  purpose: '',
  url: '',
  subject: '',
  judge: '',
  // Composite workflows contain non-repeatable steps; zero is the safe,
  // valid default for every new local plan.
  technicalRetries: '0',
  scenarios: [] as string[],
  testQuery: '',
  runs: '1',
  seed: '',
}

export type PlanFormValues = typeof PLAN_FORM_DEFAULTS

export function planFormDirty(
  current: PlanFormValues,
  initial: PlanFormValues,
) {
  return (
    current.label !== initial.label ||
    current.purpose !== initial.purpose ||
    current.url !== initial.url ||
    current.subject !== initial.subject ||
    current.judge !== initial.judge ||
    current.testQuery !== initial.testQuery ||
    current.runs !== initial.runs ||
    current.technicalRetries !== initial.technicalRetries ||
    current.seed !== initial.seed ||
    current.scenarios.join('\u0000') !== initial.scenarios.join('\u0000')
  )
}

export function LocalPlanCreatePage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [label, setLabel] = useState(PLAN_FORM_DEFAULTS.label)
  const [purpose, setPurpose] = useState(PLAN_FORM_DEFAULTS.purpose)
  const [url, setUrl] = useState(PLAN_FORM_DEFAULTS.url)
  const [subject, setSubject] = useState(PLAN_FORM_DEFAULTS.subject)
  const [judge, setJudge] = useState(PLAN_FORM_DEFAULTS.judge)
  const [scenarios, setScenarios] = useState<string[]>(
    PLAN_FORM_DEFAULTS.scenarios,
  )
  const [testQuery, setTestQuery] = useState(PLAN_FORM_DEFAULTS.testQuery)
  const [runs, setRuns] = useState(PLAN_FORM_DEFAULTS.runs)
  const [technicalRetries, setTechnicalRetries] = useState(
    PLAN_FORM_DEFAULTS.technicalRetries,
  )
  const [seed, setSeed] = useState(PLAN_FORM_DEFAULTS.seed)
  const [loading, setLoading] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const initialValues = useRef<PlanFormValues>({ ...PLAN_FORM_DEFAULTS })

  const loadCatalog = useCallback(async (source: DashboardDataBridge) => {
    const loaded = catalogValue(await source.getCatalog())
    const firstModel = loaded.models[0] ? modelKey(loaded.models[0]) : ''
    setCatalog(loaded)
    setUrl((current) => current || loaded.url)
    setSubject((current) => current || firstModel)
    initialValues.current = {
      ...initialValues.current,
      url: initialValues.current.url || loaded.url,
      subject: initialValues.current.subject || firstModel,
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then(async (next) => {
        if (cancelled) return
        setBridge(next)
        if (next.mode !== 'local')
          throw new Error(
            'Local plans are available only in the local dashboard',
          )
        await loadCatalog(next)
      })
      .catch((cause) => {
        if (!cancelled) setError(errorText(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [loadCatalog])

  // Audit PN-17: the plan page can retry a catalog load without a reload.
  const refreshCatalog = async () => {
    if (bridge?.mode !== 'local') return
    setLoading(true)
    setError(null)
    try {
      await loadCatalog(bridge)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setLoading(false)
    }
  }

  const selectedSubject = catalog?.models.find(
    (item) => modelKey(item) === subject,
  )
  const selectedJudge = catalog?.models.find((item) => modelKey(item) === judge)
  const groupedModels = useMemo(
    () => modelGroups(catalog?.models ?? []),
    [catalog],
  )
  const modelOptions = groupedModels.map((group) => ({
    provider: group.provider,
    models: group.models.map((item) => ({
      label: item.model,
      value: modelKey(item),
    })),
  }))
  const runsPerTest = Math.max(1, Number(runs) || 1)
  const retryCount = Math.max(0, Number(technicalRetries) || 0)
  const plannedRuns = scenarios.length * runsPerTest
  const hasPlanLabel = label.trim().length > 0
  const canCreate =
    !loading &&
    !submitting &&
    Boolean(selectedSubject) &&
    scenarios.length > 0 &&
    hasPlanLabel

  const dirty = planFormDirty(
    {
      label,
      purpose,
      url,
      subject,
      judge,
      scenarios,
      testQuery,
      runs,
      technicalRetries,
      seed,
    },
    initialValues.current,
  )

  const dirtyNavigation = useDirtyNavigation(dirty && !submitting)

  const create = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (
      bridge?.mode !== 'local' ||
      !selectedSubject ||
      scenarios.length === 0 ||
      !hasPlanLabel
    ) {
      setError(
        'Add a plan label, choose an execution model and select at least one test.',
      )
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const plan = await bridge.createPlan({
        label: label.trim(),
        purpose: purpose.trim(),
        url,
        model: selectedSubject.model,
        provider: selectedSubject.provider,
        judge_model: selectedJudge?.model ?? '',
        judge_provider: selectedJudge?.provider ?? '',
        scenarios,
        runs: runsPerTest,
        technical_retries: retryCount,
        seed: seed ? Number(seed) : null,
      })
      initialValues.current = {
        label: label.trim(),
        purpose: purpose.trim(),
        url,
        subject,
        judge,
        scenarios,
        testQuery,
        runs,
        technicalRetries,
        seed,
      }
      window.location.hash = hashForPlan(plan.id)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <>
      <DiscardDraftDialog
        open={dirtyNavigation.pendingHash !== null}
        warning={dirtyNavigation.warning}
        onKeep={dirtyNavigation.cancelNavigation}
        onDiscard={dirtyNavigation.confirmNavigation}
      />
      <DashboardPageActions active="plans" />
      <div className="ds-root mx-auto w-[calc(100%_-_1.5rem)] max-w-[1420px] py-8 sm:w-[calc(100%_-_3rem)] sm:py-10">
        <header className="grid items-end gap-6 border-b border-[var(--color-rule)] pb-6 lg:grid-cols-[minmax(0,1fr)_auto]">
          <div className="min-w-0">
            <p className="m-0 font-mono text-[0.68rem] font-semibold uppercase tracking-[0.08em] text-[var(--color-accent)]">
              Execution setup
            </p>
            <h1
              className="mt-2 mb-0 max-w-4xl font-mono text-xl font-semibold tracking-[-0.01em] text-ink"
              id="plan-create-title"
            >
              Create a benchmark plan
            </h1>
            <p className="mt-3 mb-0 max-w-3xl text-sm leading-6 text-[var(--color-ink-faint)]">
              Save a focused scope, capture its baseline, then run the same
              scenarios after your change to measure improvement.
            </p>
          </div>
          <nav
            className="grid min-w-[18rem] grid-cols-2 gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] max-[390px]:min-w-0"
            aria-label="Execution setup mode"
          >
            <a
              className="grid min-h-14 content-center gap-0.5 bg-panel-raised px-4 text-xs text-[var(--color-ink-faint)] no-underline transition-colors hover:text-ink"
              href={hashForWorkspace()}
              onClick={requestQuickExecution}
            >
              <strong className="font-semibold">Quick execution</strong>
              <span className="text-[0.65rem] text-ink-muted">
                One result now
              </span>
            </a>
            <span
              className="grid min-h-14 content-center gap-0.5 bg-[var(--color-surface-hover)] px-4 text-xs text-ink"
              aria-current="page"
            >
              <strong className="font-semibold">Reusable plan</strong>
              <span className="text-[0.65rem] text-ink-muted">
                Baseline + candidates
              </span>
            </span>
          </nav>
        </header>

        {error && (
          <section
            className="mt-6 rounded-lg border border-[color-mix(in_srgb,var(--color-alert)_35%,transparent)] bg-[color-mix(in_srgb,var(--color-alert)_8%,var(--surface))] p-5"
            role="alert"
          >
            <h2 className="m-0 text-sm font-semibold text-danger">
              Plan cannot be created
            </h2>
            <p className="mt-2 mb-0 text-xs leading-5 text-[var(--color-ink-faint)]">
              {error}
            </p>
          </section>
        )}

        <form
          className="mt-6 grid min-w-0 gap-px overflow-hidden rounded-[6px] border border-[var(--color-edge)] bg-[var(--color-rule)] lg:grid-cols-12"
          onSubmit={create}
        >
          <div className="min-w-0 bg-panel lg:col-span-8">
            <ExecutionSetup
              idPrefix="plan-create"
              mode="plan"
              label={label}
              purpose={purpose}
              url={url}
              subject={subject}
              judge={judge}
              modelGroups={modelOptions}
              availableScenarios={catalog?.scenarios ?? []}
              selectedScenarios={scenarios}
              query={testQuery}
              runs={runs}
              technicalRetries={technicalRetries}
              seed={seed}
              disabled={submitting}
              catalogLoading={loading}
              catalogSummary={
                loading
                  ? 'Loading local catalog'
                  : catalog
                    ? `${catalog.models.length} registered model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} tests`
                    : 'Catalog unavailable'
              }
              onRefreshCatalog={() => void refreshCatalog()}
              onLabelChange={setLabel}
              onPurposeChange={setPurpose}
              onUrlChange={setUrl}
              onSubjectChange={setSubject}
              onJudgeChange={setJudge}
              onSelectedScenariosChange={setScenarios}
              onQueryChange={setTestQuery}
              onRunsChange={setRuns}
              onTechnicalRetriesChange={setTechnicalRetries}
              onSeedChange={setSeed}
            />
          </div>

          <div className="min-w-0 bg-panel-raised lg:col-span-4">
            <ExecutionSetupReview
              mode="plan"
              status={canCreate ? 'Ready' : 'Incomplete'}
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
              url={url}
              selectedScenarios={scenarios.length}
              plannedRuns={plannedRuns}
              runsPerScenario={runsPerTest}
              technicalRetries={retryCount}
              ready={canCreate}
            >
              <p
                className="m-0 text-xs leading-5 text-ink-muted"
                id="plan-create-requirements"
              >
                {hasPlanLabel && scenarios.length > 0
                  ? 'Ready to create a draft. The scope stays editable until the baseline starts.'
                  : `Before creating: ${hasPlanLabel ? 'select at least one test' : 'add a plan label'}${hasPlanLabel || scenarios.length === 0 ? '' : ' and select at least one test'}.`}
              </p>
              <button
                className={buttonClassName({
                  variant: 'primary',
                  size: 'large',
                  className: 'w-full',
                })}
                type="submit"
                disabled={!canCreate}
                aria-describedby="plan-create-requirements"
              >
                {submitting ? 'creating…' : 'create draft plan'}
              </button>
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'large',
                  className: 'w-full no-underline',
                })}
                href={hashForPlans()}
              >
                cancel
              </a>
            </ExecutionSetupReview>
          </div>
        </form>
      </div>
    </>
  )
}

// The plan detail lives in PlanDetailPage.tsx; these names stay importable
// from here for the router and the tests.
export {
  LocalPlanDetailPage,
  PLAN_COMPARISON_TABLE_METRICS,
  PlanExecutionHistory,
  PlanLifecycle,
  PlanNonComparableAttempts,
  PlanRunHistory,
  PlanScope,
  planMetricWinnerIds,
  planReadiness,
  selectedPlanCandidate,
} from './PlanDetailPage'
