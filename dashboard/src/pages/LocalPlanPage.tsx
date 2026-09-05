import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DiscardDraftDialog } from '@/components/DiscardDraftDialog'
import {
  consumePlanScopeRequest,
  ExecutionSetup,
  ExecutionSetupFooter,
  focusFirstInvalid,
  requestQuickExecution,
  validateExecutionSetup,
} from '@/components/ExecutionSetup'
import { buttonClassName, Callout, PageHeader, Select } from '@/design-system'
import { useDirtyNavigation } from '@/hooks/use-dirty-navigation'
import {
  hashForNewPlan,
  hashForPlan,
  hashForPlans,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type JsonObject,
  type MasterTestProfile,
} from '@/lib/dashboard-data-source'
import { type PlanRequirements, planAction } from '@/lib/plan-execution'
import { Requirements } from './PlanExecutionPage'

type Model = { provider: string; model: string }
type Catalog = {
  url: string
  models: Model[]
  scenarios: string[]
  /** Markdown tests the catalog lists but plans cannot run (audit PN-06). */
  localScenarioIds: string[]
}

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
    localScenarioIds: [...localScenarioIds],
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

export function LocalPlanCreatePage({
  profileId,
  duplicateId,
  editId,
}: {
  profileId?: string
  duplicateId?: string
  editId?: string
} = {}) {
  const [templates, setTemplates] = useState<MasterTestProfile[]>([])
  const [templateId, setTemplateId] = useState<string | undefined>(profileId)
  const [requiredJudges, setRequiredJudges] = useState<string[]>([])
  const [savedId, setSavedId] = useState<string | null>(null)
  const [requirements, setRequirements] = useState<PlanRequirements | null>(
    null,
  )
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
  const [attempted, setAttempted] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const initialValues = useRef<PlanFormValues>({ ...PLAN_FORM_DEFAULTS })

  const loadCatalog = useCallback(async (source: DashboardDataBridge) => {
    const loaded = catalogValue(await source.getCatalog())
    // Audit RS-13: a scope chosen in the run-suite dialog arrives preselected.
    const handedOver = consumePlanScopeRequest().filter((id) =>
      loaded.scenarios.includes(id),
    )
    setCatalog(loaded)
    setUrl((current) => current || loaded.url)
    if (handedOver.length > 0)
      setScenarios((current) => (current.length > 0 ? current : handedOver))
    initialValues.current = {
      ...initialValues.current,
      url: initialValues.current.url || loaded.url,
      subject: initialValues.current.subject,
      scenarios:
        initialValues.current.scenarios.length > 0
          ? initialValues.current.scenarios
          : handedOver,
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
        const listed = await next.listPlans()
        setTemplates(listed.master_plan?.profiles ?? [])
        setRequiredJudges([
          ...new Set(
            (listed.master_plan?.profiles ?? []).flatMap((p) =>
              (p.cases ?? [])
                .filter((c) => c.judge_required)
                .map((c) => c.scenario_id),
            ),
          ),
        ])
        if (duplicateId || editId) {
          const source = await next.getPlan((duplicateId ?? editId) as string)
          setLabel(duplicateId ? `${source.label} copy` : source.label)
          setPurpose(source.purpose)
          setUrl(source.url)
          setScenarios(source.scenario_ids)
          setRuns(String(source.runs))
          setTechnicalRetries(String(source.technical_retries))
          setSeed(source.seed === null ? '' : String(source.seed))
          setJudge(
            source.judge_model
              ? modelKey({
                  model: source.judge_model,
                  provider: source.judge_provider,
                })
              : '',
          )
          setSubject(editId ? modelKey(source) : '')
          setTemplateId(source.template_id ?? undefined)
          initialValues.current = {
            ...initialValues.current,
            label: duplicateId ? `${source.label} copy` : source.label,
            purpose: source.purpose,
            url: source.url,
            subject: editId ? modelKey(source) : '',
            judge: source.judge_model
              ? modelKey({
                  model: source.judge_model,
                  provider: source.judge_provider,
                })
              : '',
            scenarios: source.scenario_ids,
            runs: String(source.runs),
            technicalRetries: String(source.technical_retries),
            seed: source.seed === null ? '' : String(source.seed),
          }
        } else if (profileId) {
          const template = listed.master_plan?.profiles.find(
            (p) => p.id === profileId,
          )
          if (!template) throw Error('Starting template is unavailable.')
          setLabel(template.label)
          setPurpose(template.purpose)
          setScenarios(template.scenario_ids)
          setRuns(String(template.repetitions))
          setTechnicalRetries(String(template.technical_retries))
          initialValues.current = {
            ...initialValues.current,
            label: template.label,
            purpose: template.purpose,
            scenarios: template.scenario_ids,
            runs: String(template.repetitions),
            technicalRetries: String(template.technical_retries),
          }
        }
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
  }, [loadCatalog, profileId, duplicateId, editId])

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
  const judgeRequired =
    Boolean(
      templates.find((template) => template.id === templateId)
        ?.protected_supervisor_required,
    ) || scenarios.some((id) => requiredJudges.includes(id))
  // Audit PN-05 / PN-15: the primary stays enabled; after a submit attempt
  // the pending items show inline and next to the button.
  const errors = attempted
    ? validateExecutionSetup({
        mode: 'plan',
        label,
        subject,
        selectedScenarios: scenarios,
        url,
        judge,
        judgeRequired,
      })
    : {}

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
    const runAfterSave =
      (event.nativeEvent as SubmitEvent).submitter?.getAttribute('value') ===
      'run'
    const nextErrors = validateExecutionSetup({
      mode: 'plan',
      label,
      subject,
      selectedScenarios: scenarios,
      url,
      judge,
      judgeRequired,
    })
    if (
      Object.keys(nextErrors).length > 0 ||
      bridge?.mode !== 'local' ||
      !selectedSubject
    ) {
      setAttempted(true)
      focusFirstInvalid('plan-create', nextErrors)
      if (bridge?.mode !== 'local')
        setError('Plans can only be created from the local dashboard.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const request = {
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
      }
      const plan =
        editId || savedId
          ? await bridge.updatePlan((editId ?? savedId) as string, request)
          : await bridge.createPlan({
              ...request,
              template_id: templateId,
              duplicate_of: duplicateId,
            })
      setSavedId(plan.id)
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
      if (runAfterSave) {
        const checked = await planAction<PlanRequirements>(bridge, {
          action: 'requirements',
          plan_id: plan.id,
        })
        setRequirements(checked)
        if (!checked.ready) return
        await bridge.startPlan(plan.id, 'baseline')
      }
      window.location.hash = hashForPlan(plan.id)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSubmitting(false)
    }
  }

  const summary = {
    mode: 'plan' as const,
    selectedScenarios: scenarios.length,
    runsPerScenario: runsPerTest,
    technicalRetries: retryCount,
    seed,
    subject: selectedSubject
      ? `${selectedSubject.provider} / ${selectedSubject.model}`
      : '',
    judge: selectedJudge
      ? `${selectedJudge.provider} / ${selectedJudge.model}`
      : '',
    url,
  }
  const localCount = catalog?.localScenarioIds.length ?? 0

  return (
    <>
      <DiscardDraftDialog
        open={dirtyNavigation.pendingHash !== null}
        warning={dirtyNavigation.warning}
        onKeep={dirtyNavigation.cancelNavigation}
        onDiscard={dirtyNavigation.confirmNavigation}
      />
      <DashboardPageActions active="plans" context="new plan" />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-8 md:w-[calc(100%_-_3rem)]">
        {/* Audit PN-18 / PN-25: a breadcrumb back to plans and the DS heading scale. */}
        <PageHeader
          title={
            editId ? 'edit plan' : duplicateId ? 'duplicate plan' : 'new plan'
          }
          summary="Save a focused scope, capture its baseline, then run the same tests after your change to measure the difference."
          headingId="plan-create-title"
          breadcrumb={[
            { label: 'plans', href: hashForPlans() },
            { label: 'new plan' },
          ]}
          actions={
            <a
              className={buttonClassName({
                variant: 'quiet',
                className: 'no-underline',
              })}
              href={hashForWorkspace()}
              onClick={() => requestQuickExecution()}
            >
              quick execution instead
            </a>
          }
        />
        {!duplicateId && !editId ? (
          <label className="mt-5 block text-sm" htmlFor="plan-template">
            Start from a template
            <Select
              id="plan-template"
              className="mt-2 block"
              aria-label="Start from a template"
              value={profileId ?? ''}
              onChange={(event) => {
                window.location.hash = event.target.value
                  ? `${hashForNewPlan()}/profile/${event.target.value}`
                  : hashForNewPlan()
              }}
            >
              <option value="">Blank plan</option>
              {templates.map((template) => (
                <option key={template.id} value={template.id}>
                  {template.label}
                </option>
              ))}
            </Select>
            <span className="mt-1 block text-xs text-ink-soft">
              Templates fill the initial scope and policy. Edit them and select
              the execution model and judge before saving.
            </span>
          </label>
        ) : null}
        {requirements ? <Requirements value={requirements} /> : null}
        {savedId ? (
          <Callout tone="info" title="Draft saved">
            <a href={hashForPlan(savedId)}>Open saved plan</a>
          </Callout>
        ) : null}
        <form
          className="mt-6 grid min-w-0 gap-8"
          onSubmit={create}
          noValidate
          aria-labelledby="plan-create-title"
        >
          <ExecutionSetup
            idPrefix="plan-create"
            mode="plan"
            stickyOffset="page"
            label={label}
            purpose={purpose}
            url={url}
            subject={subject}
            judge={judge}
            judgeRequired={judgeRequired}
            modelGroups={modelOptions}
            availableScenarios={catalog?.scenarios ?? []}
            localScenarioIds={catalog?.localScenarioIds ?? []}
            unavailableScenarios={
              localCount > 0
                ? {
                    ids: catalog?.localScenarioIds ?? [],
                    reason: 'Local Markdown tests are not available in plans.',
                  }
                : undefined
            }
            selectedScenarios={scenarios}
            query={testQuery}
            runs={runs}
            technicalRetries={technicalRetries}
            seed={seed}
            disabled={submitting || loading}
            catalogLoading={loading}
            catalogStatus={
              loading
                ? { tone: 'loading', text: 'loading catalog…' }
                : catalog
                  ? {
                      tone: 'ready',
                      text: `catalog ready · ${catalog.models.length} model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} test${catalog.scenarios.length === 1 ? '' : 's'}${localCount > 0 ? ` · ${localCount} local not available in plans` : ''}`,
                    }
                  : { tone: 'unavailable', text: 'catalog unavailable' }
            }
            errors={errors}
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
          {/* Audit PN-02 / RS-03: the action bar stays in view at every width. */}
          <div className="sticky bottom-0 z-10 border-t border-line bg-panel py-3">
            <ExecutionSetupFooter
              summary={summary}
              pending={Object.values(errors)}
              error={error}
              status={
                Object.keys(errors).length === 0
                  ? 'Creates a draft. The scope stays editable until the baseline starts.'
                  : null
              }
            >
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  className: 'no-underline',
                })}
                href={hashForPlans()}
              >
                cancel
              </a>
              <button
                className={buttonClassName({ variant: 'primary' })}
                type="submit"
                disabled={submitting || loading}
                aria-busy={submitting}
              >
                {submitting ? 'saving…' : 'Save draft'}
              </button>
              <button
                className={buttonClassName({ variant: 'primary' })}
                type="submit"
                value="run"
                disabled={submitting || loading}
              >
                Save and run
              </button>
            </ExecutionSetupFooter>
          </div>
        </form>
      </div>
    </>
  )
}

// The plan detail lives in PlanDetailPage.tsx; these names stay importable
// from here for the router and the tests.
export {
  executionHistoryRows,
  executionsScent,
  LocalPlanDetailPage,
  PLAN_COMPARISON_TABLE_METRICS,
  PLAN_TREND_BANDS,
  PlanComparisonLayers,
  PlanExecutionHistory,
  PlanLifecycle,
  PlanNonComparableAttempts,
  PlanRunHistory,
  PlanScope,
  planMetricWinnerIds,
  planMovementGroups,
  planProvenanceEntries,
  planProvenanceScent,
  planReadiness,
  planTrendTiles,
  selectedPlanCandidate,
} from './PlanDetailPage'
