import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
  DashboardRunProjection,
  DashboardScenarioMetricSummary,
} from '@/lib/dashboard-data-source'
import { getDashboardIiiClient } from '@/lib/iii-client'

const PLANS_LIST = 'release-control::test-plans::list'
const EXECUTION_REFERENCE = 'release-control::test-executions::reference'

export type RcExecutionSummary = {
  id: string
  campaignId: string
  planKey: string
  attempt: number
  trigger: string
  requestedBy: string | null
  label: string | null
  phase: string
  terminal: boolean
  resultState: string
  requestedAt: string
  completedAt: string | null
  error: string | null
  runCount: number
  ghRunUrl?: string | null
  reportCount: number
}

export type RcRun = {
  attemptsComplete: boolean
  scenarioId: string
  scenarioVersion: number | null
  caseId?: string | null
  status: string | null
  completion: string | null
  technical: string | null
  objectiveScore: number | null
  wallTimeMs: number | null
  totalTokens: number | null
  costSubjectUsd: number | null
  turns: number | null
  functionCalls: number | null
  functionCallErrors?: number | null
}

export type RcReference = {
  execution: RcExecutionSummary & {
    plan: Record<string, unknown>
    request: Record<string, unknown>
  }
  runs: RcRun[]
  aggregate: {
    planned_runs: number
    observed_runs: number
    completion_rate: number | null
    execution_reliability: number | null
  }
  materialized: Record<string, unknown> | null
  shards: Record<string, unknown>[]
}

type RcPlan = { recentExecutions: RcExecutionSummary[] }

function completeSum(values: Array<number | null | undefined>) {
  return values.length > 0 &&
    values.every((value) => typeof value === 'number' && Number.isFinite(value))
    ? values.reduce<number>((sum, value) => sum + (value ?? 0), 0)
    : undefined
}

function completeMean(
  values: Array<number | null | undefined>,
  divisor: number,
) {
  const sum = completeSum(values)
  return sum === undefined ? null : sum / divisor
}

function status(execution: RcExecutionSummary) {
  if (!execution.terminal)
    return execution.phase === 'cancel_requested' ? 'cancelling' : 'running'
  return execution.phase || 'unavailable'
}

function metrics(runs: RcRun[]): DashboardScenarioMetricSummary[] {
  const groups = new Map<string, RcRun[]>()
  for (const run of runs)
    groups.set(run.scenarioId, [...(groups.get(run.scenarioId) ?? []), run])
  return [...groups].map(([scenario_id, values]) => ({
    scenario_id,
    scenario_version: values[0]?.scenarioVersion ?? undefined,
    run_count: values.length,
    averages: {
      tokens: completeMean(
        values.map((run) => run.totalTokens),
        values.length,
      ),
      duration_seconds: (() => {
        const mean = completeMean(
          values.map((run) => run.wallTimeMs),
          values.length,
        )
        return mean === null ? null : mean / 1000
      })(),
      cost_usd: completeMean(
        values.map((run) => run.costSubjectUsd),
        values.length,
      ),
      function_calls: completeMean(
        values.map((run) => run.functionCalls),
        values.length,
      ),
      turns: completeMean(
        values.map((run) => run.turns),
        values.length,
      ),
    },
    samples: {
      tokens: values.filter((run) => run.totalTokens !== null).length,
      duration_seconds: values.filter((run) => run.wallTimeMs !== null).length,
      cost_usd: values.filter((run) => run.costSubjectUsd !== null).length,
      turns: values.filter((run) => run.turns !== null).length,
      function_calls: values.filter((run) => run.functionCalls !== null).length,
    },
  }))
}

export function referenceSummary(view: {
  execution: RcExecutionSummary
  runs: RcRun[]
  aggregate: RcReference['aggregate'] | null
}): DashboardExecutionSummary {
  const { execution, aggregate } = view
  const runs = view.runs.map((run) =>
    run.attemptsComplete
      ? run
      : {
          ...run,
          totalTokens: null,
          costSubjectUsd: null,
          functionCalls: null,
          functionCallErrors: null,
          turns: null,
        },
  )
  const total_tokens = completeSum(runs.map((run) => run.totalTokens))
  const wall_time_seconds = completeSum(runs.map((run) => run.wallTimeMs))
  const total_cost_usd = completeSum(runs.map((run) => run.costSubjectUsd))
  const function_calls = completeSum(runs.map((run) => run.functionCalls))
  const turns = completeSum(runs.map((run) => run.turns))
  const plan = ('plan' in execution ? execution.plan : {}) as Record<
    string,
    unknown
  >
  const subject = plan.subject as
    | { model?: string; provider?: string }
    | undefined
  return {
    id: `rc:${execution.id}`,
    label: execution.label ?? execution.planKey,
    run_id: execution.id,
    attempt: execution.attempt,
    event: execution.trigger,
    actor: execution.requestedBy ?? undefined,
    status: status(execution),
    workflow_url: execution.ghRunUrl,
    started_at: execution.requestedAt,
    completed_at: execution.completedAt ?? undefined,
    availability: execution.reportCount > 0 ? 'aggregate' : 'unavailable',
    source: { provenance: 'release-control', execution_id: execution.id },
    release_control: {
      execution_id: execution.id,
      attempt: execution.attempt,
      profile: execution.planKey,
      campaign_id: execution.campaignId,
      group_id: null,
    },
    subjects:
      subject?.model && subject.provider
        ? [
            {
              id: subject.model,
              model: subject.model,
              provider: subject.provider,
              scenarios: metrics(runs).map((metric) => ({
                id: metric.scenario_id,
                scenario_version: metric.scenario_version,
                case_id:
                  runs.find((run) => run.scenarioId === metric.scenario_id)
                    ?.caseId ?? undefined,
              })),
            },
          ]
        : [],
    scenario_metrics: metrics(runs),
    totals: {
      expected_reports: aggregate?.planned_runs ?? null,
      received_reports: aggregate?.observed_runs ?? null,
      total_tokens: total_tokens ?? null,
      wall_time_seconds:
        wall_time_seconds === undefined ? null : wall_time_seconds / 1000,
      total_cost_usd: total_cost_usd ?? null,
      technical_failures:
        runs.length > 0
          ? runs.filter((run) => run.technical === 'technical_invalid').length
          : null,
      scenario_pass_rate: aggregate?.completion_rate ?? null,
      report_coverage:
        aggregate && aggregate.planned_runs > 0
          ? aggregate.observed_runs / aggregate.planned_runs
          : null,
      function_calls: function_calls ?? null,
      turns: turns ?? null,
      function_call_errors:
        completeSum(runs.map((run) => run.functionCallErrors)) ?? null,
    },
  }
}

/** Mean objective score over the runs RC actually measured; absent remains absent. */
function finiteMean(scores: Array<number | null | undefined>) {
  const measured = scores.filter(
    (score): score is number =>
      typeof score === 'number' && Number.isFinite(score),
  )
  return measured.length === 0
    ? null
    : measured.reduce((sum, score) => sum + score, 0) / measured.length
}

export function objectiveScore(view: RcReference | DashboardExecutionDetail) {
  if ('aggregate' in view) {
    const reference = view as RcReference
    return finiteMean(reference.runs.map((run) => run.objectiveScore))
  }
  return finiteMean(
    view.reports.flatMap(
      (entry) =>
        entry.report?.scenarios.flatMap((scenario) =>
          scenario.runs.map((run) => run.objective_score),
        ) ?? [],
    ),
  )
}

function localAttemptsComplete(run: DashboardRunProjection) {
  const unavailable = run.efficiency?.unavailable
  return (
    run.efficiency != null &&
    !(
      unavailable &&
      typeof unavailable === 'object' &&
      'retry_efficiency' in unavailable
    ) &&
    (run.efficiency.technical_attempts == null ||
      run.efficiency.technical_attempts ===
        (run.retry_attempts?.length ?? 0) + 1)
  )
}

// RC counts cache tokens across every retained technical attempt too.
function inclusiveTokens(run: DashboardRunProjection): number | null {
  if (!localAttemptsComplete(run)) return null
  const attempts = [run, ...(run.retry_attempts ?? [])]
  const cache = completeSum(
    attempts.map((attempt) => {
      const metrics = attempt.metrics
      if (metrics?.complete !== true || !metrics.totals) return null
      return (
        completeSum([
          metrics.totals.cache_read_tokens ?? 0,
          metrics.totals.cache_write_tokens ?? 0,
        ]) ?? null
      )
    }),
  )
  const nonCache = run.efficiency?.total_tokens
  return typeof nonCache === 'number' && cache !== undefined
    ? nonCache + cache
    : null
}

export function comparisonSummary(
  view: DashboardExecutionDetail,
): DashboardExecutionSummary {
  const runs = view.reports.flatMap(
    (entry) =>
      entry.report?.scenarios.flatMap((scenario) =>
        scenario.runs.map((run) => ({
          attemptsComplete: localAttemptsComplete(run),
          scenarioId: entry.scenario_id,
          scenarioVersion:
            typeof scenario.scenario_version === 'number'
              ? scenario.scenario_version
              : null,
          caseId:
            typeof scenario.case_id === 'string' ? scenario.case_id : null,
          status: run.status,
          technical: run.technical,
          completion: run.completion,
          objectiveScore: run.objective_score,
          wallTimeMs: run.wall_time_ms ?? null,
          totalTokens: inclusiveTokens(run),
          costSubjectUsd: localAttemptsComplete(run)
            ? (run.cost?.subject_usd ?? null)
            : null,
          functionCalls: localAttemptsComplete(run)
            ? (run.efficiency?.function_calls ?? null)
            : null,
          functionCallErrors: localAttemptsComplete(run)
            ? (run.efficiency?.function_call_errors ?? null)
            : null,
          turns:
            localAttemptsComplete(run) &&
            typeof run.efficiency?.root_turns === 'number'
              ? run.efficiency.root_turns +
                (typeof run.efficiency.child_turns === 'number'
                  ? run.efficiency.child_turns
                  : 0)
              : null,
        })),
      ) ?? [],
  )
  if (runs.length === 0)
    return {
      ...view,
      totals: { ...view.totals, total_tokens: null, total_cost_usd: null },
    }
  return {
    ...view,
    scenario_metrics: metrics(runs),
    totals: {
      ...view.totals,
      total_tokens: completeSum(runs.map((run) => run.totalTokens)) ?? null,
      tokens_per_completion: null,
      failed_attempt_tokens: null,
      total_cost_usd:
        completeSum(runs.map((run) => run.costSubjectUsd)) ?? null,
      wall_time_seconds: completeMean(
        runs.map((run) => run.wallTimeMs),
        1000,
      ),
      function_calls: completeSum(runs.map((run) => run.functionCalls)) ?? null,
      function_call_errors:
        completeSum(runs.map((run) => run.functionCallErrors)) ?? null,
      turns: completeSum(runs.map((run) => run.turns)) ?? null,
      technical_failures: runs.filter(
        (run) => run.technical === 'technical_invalid',
      ).length,
    },
  }
}

export async function listReleaseControlExecutions(): Promise<
  DashboardExecutionSummary[]
> {
  const { plans } = await getDashboardIiiClient().then((client) =>
    client.trigger<{ plans: RcPlan[] }>(PLANS_LIST, {}),
  )
  return plans
    .flatMap((plan) => plan.recentExecutions)
    .map((execution) =>
      referenceSummary({
        execution,
        runs: [],
        aggregate: null,
      }),
    )
}

export async function getReleaseControlReference(
  id: string,
): Promise<RcReference> {
  return getDashboardIiiClient().then((client) =>
    client.trigger<RcReference>(EXECUTION_REFERENCE, {
      executionId: id.replace(/^rc:/, ''),
    }),
  )
}
