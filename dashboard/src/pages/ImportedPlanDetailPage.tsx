import { useCallback, useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import {
  ExecutionSetup,
  ExecutionSetupFooter,
} from '@/components/ExecutionSetup'
import { PrimaryMetricsView } from '@/components/PrimaryMetricsView'
import {
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  Dialog,
  EmptyState,
  PageHeader,
  StatusBadge,
} from '@/design-system'
import {
  hashForExecution,
  hashForPlans,
  hashForTestHistory,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type JsonObject,
  type LocalPlan,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  formatDate,
  statusCopy,
} from '@/lib/execution-view'
import { loadExecutionSummaries } from '@/lib/plan-comparison'
import {
  exportReleaseControlHistory,
  getImportedReference,
  listImportedExecutions,
  localReferencePrimaryMetrics,
  type RcReference,
  referencePrimaryMetrics,
  referenceSummary,
} from '@/lib/release-control-reference'
import { watchExecution } from '@/lib/watch-execution'

function object(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonObject)
    : {}
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

function count(value: unknown) {
  return typeof value === 'number' && Number.isInteger(value) ? value : null
}

function model(value: unknown) {
  const entry = object(value)
  return { provider: text(entry.provider), model: text(entry.model) }
}

function frozenSetup(reference: RcReference) {
  const plan = object(reference.execution.plan)
  const materialized = object(reference.materialized)
  const profile = object(materialized.profile)
  const campaigns = Array.isArray(materialized.campaigns)
    ? materialized.campaigns.map(object)
    : []
  const groups = campaigns.flatMap((campaign) =>
    Array.isArray(campaign.groups) ? campaign.groups.map(object) : [],
  )
  const scenarios = [
    ...new Set(
      groups
        .flatMap((group) =>
          Array.isArray(group.scenarios) ? group.scenarios : [],
        )
        .filter((id): id is string => typeof id === 'string'),
    ),
  ]
  const seeds = new Map<string, string>()
  for (const shard of reference.shards) {
    if (!Array.isArray(shard.runs)) continue
    for (const candidate of shard.runs.map(object)) {
      const scenario = text(candidate.scenario_id)
      const seed = candidate.seed
      if (scenario && (typeof seed === 'number' || typeof seed === 'string'))
        seeds.set(scenario, String(seed))
    }
  }
  return {
    subject: model(plan.subject),
    judge: model(plan.judge),
    scenarios,
    repetitions: count(profile.repetitions),
    technicalRetries: count(profile.technical_retries),
    seeds,
  }
}

function modelValue(value: { provider: string; model: string }) {
  return value.provider && value.model
    ? `${value.provider}\n${value.model}`
    : ''
}

function modelLabel(value: { provider: string; model: string }) {
  return value.provider && value.model
    ? `${value.provider} / ${value.model}`
    : ''
}

function scenarioLink(
  scenarioId: string,
  referenceId: string,
  candidateId: string,
) {
  const params = new URLSearchParams({
    reference: referenceId,
    candidate: candidateId,
  })
  return `${hashForTestHistory(scenarioId)}?${params}`
}

/** Detail for a history imported into the local Harness store. */
export function ImportedPlanDetailPage({ planId }: { planId: string }) {
  const planKey = planId
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [history, setHistory] = useState<DashboardExecutionSummary[]>([])
  const [localPlans, setLocalPlans] = useState<LocalPlan[]>([])
  const [localSummaries, setLocalSummaries] = useState<
    Record<string, DashboardExecutionSummary>
  >({})
  const [referenceId, setReferenceId] = useState<string | null>(null)
  const [candidateId, setCandidateId] = useState<string | null>(null)
  const [reference, setReference] = useState<RcReference | null>(null)
  const [candidate, setCandidate] = useState<DashboardExecutionDetail | null>(
    null,
  )
  const [activePlan, setActivePlan] = useState<LocalPlan | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [starting, setStarting] = useState(false)
  const [updating, setUpdating] = useState(false)
  const [localUrl, setLocalUrl] = useState('Current local Harness')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setError(null)
    const next = bridge ?? (await getDashboardDataBridge())
    setBridge(next)
    const [remote, local] = await Promise.all([
      listImportedExecutions(),
      next.listPlans(),
    ])
    const imported = await next.getPlan(planId)
    if (imported.origin !== 'remote')
      throw new Error('This plan is not an imported history.')
    const executions = remote
      .filter((execution) => imported.execution_ids.includes(execution.id))
      .sort((left, right) =>
        (right.started_at ?? '').localeCompare(left.started_at ?? ''),
      )
    setHistory(executions)
    setReferenceId((current) =>
      current && executions.some((execution) => execution.id === current)
        ? current
        : (executions[0]?.id ?? null),
    )
    const executionIds = new Set(
      executions.map((execution) => execution.run_id),
    )
    const related = local.plans
      .filter((plan): plan is LocalPlan => plan.origin === 'local')
      .filter(
        (plan) =>
          plan.reference_execution_id &&
          executionIds.has(plan.reference_execution_id),
      )
    setLocalPlans(related)
    setActivePlan(
      related.find((plan) =>
        ['baseline_running', 'candidate_running'].includes(plan.state),
      ) ?? null,
    )
    const localIds = related.flatMap((plan) => [
      plan.baseline_execution_id ?? '',
      ...plan.candidate_execution_ids,
      ...plan.incomplete_execution_ids,
      plan.last_attempt_id ?? '',
    ])
    setLocalSummaries(
      await loadExecutionSummaries(next.listExecutions, localIds),
    )
  }, [bridge, planId])

  useEffect(() => {
    void load()
      .catch((cause) =>
        setError(cause instanceof Error ? cause.message : String(cause)),
      )
      .finally(() => setLoading(false))
  }, [load])

  const candidateIds = useMemo(
    () =>
      localPlans
        .flatMap((plan) => [
          plan.baseline_execution_id ?? '',
          ...plan.candidate_execution_ids,
          ...plan.incomplete_execution_ids,
          plan.last_attempt_id ?? '',
        ])
        .filter((id, index, ids) => id && ids.indexOf(id) === index),
    [localPlans],
  )

  useEffect(() => {
    setCandidateId((current) =>
      current && candidateIds.includes(current)
        ? current
        : (candidateIds.at(-1) ?? null),
    )
  }, [candidateIds])

  useEffect(() => {
    let current = true
    setReference(null)
    if (!referenceId) return
    void getImportedReference(referenceId)
      .then((value) => {
        if (current) {
          setReference(value)
          setHistory((items) =>
            items.map((item) =>
              item.id === value.execution.local_id
                ? referenceSummary(value)
                : item,
            ),
          )
        }
      })
      .catch((cause) => {
        if (current)
          setError(cause instanceof Error ? cause.message : String(cause))
      })
    return () => {
      current = false
    }
  }, [referenceId])

  useEffect(() => {
    let current = true
    setCandidate(null)
    if (!bridge || !candidateId) return
    const refresh = async () => {
      const detail = await bridge.getExecution(candidateId)
      if (current) setCandidate(detail)
    }
    void refresh().catch((cause) => {
      if (current)
        setError(cause instanceof Error ? cause.message : String(cause))
    })
    const stop = watchExecution(bridge, candidateId, refresh)
    return () => {
      current = false
      stop()
    }
  }, [bridge, candidateId])

  useEffect(() => {
    const executionId = activePlan?.last_attempt_id
    if (!bridge || !executionId) return
    let current = true
    let settled = false
    const refresh = async () => {
      const detail = await bridge.getExecution(executionId)
      if (
        current &&
        !settled &&
        !['running', 'queued', 'cancelling'].includes(detail.status)
      ) {
        settled = true
        await load()
      }
    }
    const safeRefresh = () =>
      refresh().catch((cause) => {
        if (current)
          setError(cause instanceof Error ? cause.message : String(cause))
      })
    void safeRefresh()
    const stop = watchExecution(bridge, executionId, safeRefresh)
    return () => {
      current = false
      stop()
    }
  }, [activePlan?.last_attempt_id, bridge, load])

  const frozen = reference ? frozenSetup(reference) : null
  const selectedLocalPlan = candidateId
    ? localPlans.find((plan) =>
        [
          plan.baseline_execution_id,
          ...plan.candidate_execution_ids,
          ...plan.incomplete_execution_ids,
          plan.last_attempt_id,
        ].includes(candidateId),
      )
    : undefined
  const replayable = Boolean(
    frozen &&
      frozen.scenarios.length > 0 &&
      frozen.repetitions !== null &&
      frozen.technicalRetries !== null &&
      frozen.scenarios.every((scenario) => frozen.seeds.has(scenario)),
  )
  const referenceMetrics = useMemo(
    () => (reference ? referencePrimaryMetrics(reference) : null),
    [reference],
  )
  const candidateMetrics = useMemo(() => {
    if (!candidate) return null
    const compatible = Boolean(
      reference?.execution.terminal &&
        !['running', 'queued', 'cancelling'].includes(candidate.status) &&
        selectedLocalPlan?.reference_execution_id === reference.execution.id &&
        selectedLocalPlan.technical_retries === frozen?.technicalRetries &&
        !selectedLocalPlan.reference_differences?.length,
    )
    return localReferencePrimaryMetrics(candidate, compatible)
  }, [candidate, reference, selectedLocalPlan, frozen?.technicalRetries])

  const openRunDialog = async () => {
    if (!bridge || !reference) return
    setDialogOpen(true)
    try {
      const catalog = await bridge.getCatalog()
      if (typeof catalog.url === 'string') setLocalUrl(catalog.url)
    } catch {
      setLocalUrl('Current local Harness')
    }
  }

  const runReference = async () => {
    if (!bridge || !reference) return
    setStarting(true)
    setError(null)
    try {
      const imported = (await bridge.planControl({
        action: 'reproduce_reference',
        reference_execution_id: reference.execution.id,
        label: reference.execution.label || `${planKey} local reproduction`,
        subject: object(reference.execution.plan).subject,
        judge: object(reference.execution.plan).judge,
        materialized: reference.materialized,
        shards: reference.shards,
      })) as LocalPlan
      const started = await bridge.startPlan(
        imported.id,
        imported.baseline_execution_id ? 'candidate' : 'baseline',
      )
      setActivePlan(started)
      setCandidateId(started.last_attempt_id)
      setDialogOpen(false)
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setStarting(false)
    }
  }

  const updateHistory = async () => {
    if (!bridge) return
    setUpdating(true)
    setError(null)
    try {
      const imported = await bridge.getPlan(planId)
      if (imported.origin !== 'remote')
        throw new Error('This plan is not imported history.')
      const history = await exportReleaseControlHistory(
        imported.source.plan_key,
      )
      await bridge.planControl({ action: 'import_history', history })
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setUpdating(false)
    }
  }

  const cancel = async () => {
    if (!bridge || !activePlan?.last_attempt_id) return
    try {
      await bridge.planControl({
        action: 'cancel',
        execution_id: activePlan.last_attempt_id,
      })
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const planName = reference
    ? text(object(reference.execution.plan).name) || planKey
    : planKey

  return (
    <>
      <DashboardPageActions
        active="plans"
        context={planName}
        actionsLabel="Reference plan actions"
        actions={null}
      />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title={planName}
          context="Reference: Release Control"
          summary="Shared execution history beside results produced by your current local Harness."
          actions={
            <>
              <button
                type="button"
                className={buttonClassName({ variant: 'secondary' })}
                disabled={updating}
                onClick={() => void updateHistory()}
              >
                {updating ? 'updating…' : 'update history'}
              </button>
              <a
                className={buttonClassName({ variant: 'quiet' })}
                href={hashForPlans()}
              >
                back to plans
              </a>
            </>
          }
        />

        {error ? (
          <Callout
            className="mt-5"
            tone="danger"
            title="Plan action unavailable"
          >
            {error}
          </Callout>
        ) : null}

        {loading ? (
          <p className="mt-5 font-mono text-xs text-ink-muted">
            loading plan history…
          </p>
        ) : history.length === 0 ? (
          <EmptyState
            className="mt-5"
            title="No Release Control history found"
            description="Keep an authenticated Release Control tab connected to this local Engine, then reload the plan."
          />
        ) : (
          <>
            <section
              className="mt-5 grid gap-3"
              aria-labelledby="reference-history-title"
            >
              <div className="flex flex-wrap items-end justify-between gap-3">
                <div>
                  <h2
                    id="reference-history-title"
                    className="m-0 text-base text-ink"
                  >
                    Execution history
                  </h2>
                  <p className="m-0 mt-1 text-xs text-ink-soft">
                    Shared Release Control references and results kept on this
                    computer.
                  </p>
                </div>
                <button
                  type="button"
                  className={buttonClassName({ variant: 'primary' })}
                  disabled={
                    !reference || !replayable || starting || Boolean(activePlan)
                  }
                  onClick={() => void openRunDialog()}
                >
                  run reference locally
                </button>
                {activePlan?.last_attempt_id ? (
                  <span className="flex items-center gap-2 font-mono text-xs text-ink-soft">
                    local run in progress
                    <a
                      className={buttonClassName({
                        variant: 'quiet',
                        size: 'compact',
                      })}
                      href={hashForExecution(activePlan.last_attempt_id)}
                    >
                      open
                    </a>
                    <button
                      type="button"
                      className={buttonClassName({
                        variant: 'quiet',
                        size: 'compact',
                      })}
                      onClick={() => void cancel()}
                    >
                      cancel
                    </button>
                  </span>
                ) : null}
              </div>
              {reference && !replayable ? (
                <Callout
                  tone="warning"
                  title="This reference cannot be replayed locally"
                >
                  The retained materialization does not contain every scenario,
                  seed and execution-policy field required by the local runner.
                  Its shared result remains available for review.
                </Callout>
              ) : null}
              <DataTable
                caption={`${planName} shared and local history`}
                collapse
                minWidth="52rem"
              >
                <thead>
                  <tr>
                    <th scope="col">Source</th>
                    <th scope="col">Execution</th>
                    <th scope="col">Result</th>
                    <th scope="col">Reports</th>
                    <th scope="col">Completed</th>
                    <th scope="col">Compare</th>
                  </tr>
                </thead>
                <tbody>
                  {history.map((execution) => (
                    <DataTableRow
                      key={execution.id}
                      aria-selected={execution.id === referenceId}
                    >
                      <td
                        data-label="Source"
                        className="font-mono text-label text-ink-muted"
                      >
                        Release Control
                      </td>
                      <td data-label="Execution" className="font-mono text-xs">
                        {execution.label || execution.run_id}
                      </td>
                      <td data-label="Result">
                        <StatusBadge
                          {...statusCopy(buildExecutionPresentation(execution))}
                          label={execution.status.replaceAll('_', ' ')}
                        />
                      </td>
                      <td data-label="Reports" className="font-mono text-xs">
                        {execution.totals?.received_reports ?? '—'}/
                        {execution.totals?.expected_reports ?? '—'}
                      </td>
                      <td
                        data-label="Completed"
                        className="font-mono text-xs text-ink-muted"
                      >
                        {formatDate(
                          execution.completed_at ?? execution.started_at ?? '',
                        )}
                      </td>
                      <td data-label="Compare">
                        <button
                          type="button"
                          className={buttonClassName({
                            variant:
                              execution.id === referenceId
                                ? 'primary'
                                : 'quiet',
                            size: 'compact',
                          })}
                          onClick={() => setReferenceId(execution.id)}
                        >
                          {execution.id === referenceId
                            ? 'Reference A'
                            : 'use as A'}
                        </button>
                      </td>
                    </DataTableRow>
                  ))}
                  {Object.values(localSummaries)
                    .sort((left, right) =>
                      (right.started_at ?? '').localeCompare(
                        left.started_at ?? '',
                      ),
                    )
                    .map((execution) => (
                      <DataTableRow
                        key={execution.id}
                        href={hashForExecution(execution.id)}
                        aria-selected={execution.id === candidateId}
                      >
                        <td
                          data-label="Source"
                          className="font-mono text-label text-ink-muted"
                        >
                          Local
                        </td>
                        <td
                          data-label="Execution"
                          className="font-mono text-xs"
                        >
                          {execution.label || execution.id}
                        </td>
                        <td data-label="Result">
                          <StatusBadge
                            {...statusCopy(
                              buildExecutionPresentation(execution),
                            )}
                          />
                        </td>
                        <td data-label="Reports" className="font-mono text-xs">
                          {execution.totals?.received_reports ?? '—'}/
                          {execution.totals?.expected_reports ?? '—'}
                        </td>
                        <td
                          data-label="Completed"
                          className="font-mono text-xs text-ink-muted"
                        >
                          {formatDate(
                            execution.completed_at ??
                              execution.started_at ??
                              '',
                          )}
                        </td>
                        <td data-label="Compare">
                          <button
                            type="button"
                            className={buttonClassName({
                              variant:
                                execution.id === candidateId
                                  ? 'primary'
                                  : 'quiet',
                              size: 'compact',
                            })}
                            onClick={(event) => {
                              event.preventDefault()
                              event.stopPropagation()
                              setCandidateId(execution.id)
                            }}
                          >
                            {execution.id === candidateId
                              ? 'Candidate B'
                              : 'use as B'}
                          </button>
                        </td>
                      </DataTableRow>
                    ))}
                </tbody>
              </DataTable>
            </section>

            {referenceMetrics ? (
              <section
                className="mt-5"
                aria-labelledby="plan-reference-comparison"
              >
                <div className="mb-3 flex flex-wrap items-end justify-between gap-3">
                  <div>
                    <h2
                      id="plan-reference-comparison"
                      className="m-0 text-base text-ink"
                    >
                      Reference and local candidate
                    </h2>
                    <p className="m-0 mt-1 text-xs text-ink-soft">
                      Release Control ledger A · native local Results B
                    </p>
                    <p className="m-0 mt-1 text-xs text-ink-muted">
                      RC retains total tokens, not the input/output/cache split;
                      its subject-only cost is not compared with total spend.
                    </p>
                  </div>
                  {candidateId ? (
                    <a
                      className={buttonClassName({ variant: 'quiet' })}
                      href={hashForExecution(candidateId)}
                    >
                      open local execution
                    </a>
                  ) : null}
                </div>
                <PrimaryMetricsView
                  baseline={referenceMetrics}
                  candidate={candidateMetrics ?? undefined}
                  baselineLabel={
                    reference?.execution.label || 'Release Control'
                  }
                  candidateLabel={candidate?.label || 'Local'}
                  candidateExecutionId={candidateId ?? undefined}
                />
                {selectedLocalPlan?.reference_execution_id &&
                selectedLocalPlan.reference_execution_id !==
                  reference?.execution.id ? (
                  <Callout tone="info">
                    This candidate was run from reference{' '}
                    {selectedLocalPlan.reference_execution_id}. You are
                    comparing it with {reference?.execution.id}.
                  </Callout>
                ) : null}
                {selectedLocalPlan?.reference_differences?.length ? (
                  <Callout
                    tone="warning"
                    title="Differences from the original reference"
                  >
                    <ul className="m-0 pl-5">
                      {selectedLocalPlan.reference_differences.map(
                        (difference) => (
                          <li key={difference}>{difference}</li>
                        ),
                      )}
                    </ul>
                  </Callout>
                ) : null}
                {reference && candidateId && frozen ? (
                  <nav
                    className="mt-3 flex flex-wrap gap-2"
                    aria-label="Scenario comparisons"
                  >
                    {frozen.scenarios.map((scenario) => (
                      <a
                        key={scenario}
                        className={buttonClassName({
                          variant: 'quiet',
                          size: 'compact',
                        })}
                        href={scenarioLink(
                          scenario,
                          reference.execution.id,
                          candidateId,
                        )}
                      >
                        {scenario}
                      </a>
                    ))}
                  </nav>
                ) : null}
              </section>
            ) : null}
          </>
        )}
      </div>

      <Dialog
        open={dialogOpen}
        onClose={() => !starting && setDialogOpen(false)}
        size="lg"
        tall
        kicker="Reference: Release Control"
        title="Run this reference locally"
        description="The scenario list, repetitions, seeds and retry policy come from the reference. The run uses your current local scenario implementations and reports differences."
        footer={
          frozen ? (
            <ExecutionSetupFooter
              summary={{
                mode: 'plan',
                selectedScenarios: frozen.scenarios.length,
                runsPerScenario: frozen.repetitions ?? 0,
                technicalRetries: frozen.technicalRetries ?? 0,
                seed: 'frozen per test',
                subject: modelLabel(frozen.subject),
                judge: modelLabel(frozen.judge),
                url: localUrl,
              }}
              error={error}
              status="This creates a local plan and keeps every result on this computer."
            >
              <button
                type="button"
                className={buttonClassName({ variant: 'secondary' })}
                disabled={starting || !replayable}
                onClick={() => setDialogOpen(false)}
              >
                cancel
              </button>
              <button
                type="button"
                className={buttonClassName({ variant: 'primary' })}
                disabled={starting}
                aria-busy={starting}
                onClick={() => void runReference()}
              >
                {starting ? 'starting…' : 'run on current Harness'}
              </button>
            </ExecutionSetupFooter>
          ) : null
        }
      >
        {frozen ? (
          <div className="grid gap-5">
            <Callout tone="info" title="Frozen reference">
              Seeds:{' '}
              {[...frozen.seeds.entries()]
                .map(([scenario, seed]) => `${scenario} ${seed}`)
                .join(' · ') || 'unavailable'}
            </Callout>
            <ExecutionSetup
              idPrefix="rc-reference"
              mode="plan"
              stickyOffset="dialog"
              label={reference?.execution.label || planKey}
              purpose="Local reproduction of a shared Release Control execution."
              url={localUrl}
              subject={modelValue(frozen.subject)}
              judge={modelValue(frozen.judge)}
              judgeRequired={Boolean(modelValue(frozen.judge))}
              modelGroups={[frozen.subject, frozen.judge]
                .filter((entry) => entry.provider && entry.model)
                .map((entry) => ({
                  provider: entry.provider,
                  models: [{ label: entry.model, value: modelValue(entry) }],
                }))}
              availableScenarios={frozen.scenarios}
              selectedScenarios={frozen.scenarios}
              query=""
              runs={
                frozen.repetitions === null ? '' : String(frozen.repetitions)
              }
              technicalRetries={
                frozen.technicalRetries === null
                  ? ''
                  : String(frozen.technicalRetries)
              }
              seed=""
              disabled
              catalogStatus={{
                tone: 'ready',
                text: 'Frozen Release Control parameters · current local Harness',
              }}
              onLabelChange={() => undefined}
              onPurposeChange={() => undefined}
              onUrlChange={() => undefined}
              onSubjectChange={() => undefined}
              onJudgeChange={() => undefined}
              onSelectedScenariosChange={() => undefined}
              onQueryChange={() => undefined}
              onRunsChange={() => undefined}
              onTechnicalRetriesChange={() => undefined}
              onSeedChange={() => undefined}
            />
          </div>
        ) : null}
      </Dialog>
    </>
  )
}
