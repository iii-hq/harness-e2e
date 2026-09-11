import type {
  DashboardExecutionDetail,
  DashboardExecutionSummary,
  DashboardRunProjection,
  DashboardScenarioMetricSummary,
} from '@/lib/dashboard-data-source'
import { getDashboardDataBridge } from '@/lib/dashboard-data-source'
import { getDashboardIiiClient } from '@/lib/iii-client'
import {
  buildPrimaryMetrics,
  buildPrimaryMetricsFromValues,
  type MetricId,
  type PrimaryMetrics,
  type PrimaryTestValues,
} from '@/lib/primary-metrics'
import type { TestObservation } from '@/lib/test-catalog'


export type RcExecutionSummary = {
  id: string
  /** Local copied execution id; preserves the offline route identity. */
  local_id?: string
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
  id?: string
  attemptsComplete: boolean
  scenarioId: string
  scenarioVersion: number | null
  caseId?: string | null
  seed?: string | null
  repetition?: number | null
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
  record?: {
    efficiency?: {
      input_tokens?: number | null
      output_tokens?: number | null
      cache_read_tokens?: number | null
      cache_write_tokens?: number | null
      function_call_errors?: number | null
    } | null
    cost?: { total_usd?: number | null; subject_usd?: number | null } | null
  } | null
  cohortSha256?: string
  capturedAt?: string
  identity?: {
    definitionSha256?: string | null
    harnessVersion?: string | null
    subjectProvider?: string | null
    subjectModel?: string | null
    judgeProvider?: string | null
    judgeModel?: string | null
    resultContractSha256?: string | null
    scoringProfileSha256?: string | null
  }
}

export type RcReference = {
  execution: RcExecutionSummary & {
    plan: Record<string, unknown> | null
    request: Record<string, unknown> | null
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

export type RcHistoryPlan = { key: string; active: boolean; updated_at: string }

export async function discoverReleaseControlHistory(): Promise<RcHistoryPlan[]> {
  const client = await getDashboardIiiClient()
  const plans: RcHistoryPlan[] = []
  let after: string | undefined
  do {
    const page = await client.trigger<{ plans: RcHistoryPlan[]; next_after: string | null }>(
      'release-control::test-plans::history-list', { after, limit: 100 }, { namespace: 'default' },
    )
    plans.push(...page.plans)
    after = page.next_after ?? undefined
  } while (after)
  return plans
}

export async function exportReleaseControlHistory(planKey: string) {
  return (await getDashboardIiiClient()).trigger<{ json: string; sha256: string }>(
    'release-control::test-plans::export', { planKey }, { namespace: 'default' },
  )
}


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

function normalizedSeed(value: unknown) {
  if (typeof value === 'number')
    return Number.isSafeInteger(value) && value >= 0 ? String(value) : null
  if (typeof value !== 'string' || !/^\d+$/.test(value)) return null
  try {
    return BigInt(value).toString()
  } catch {
    return null
  }
}

function primaryIdentity(values: {
  caseId: unknown
  seed: unknown
  repetition: unknown
  definitionSha256: unknown
  resultContractSha256: unknown
  scoringProfileSha256: unknown
  subjectProvider: unknown
  subjectModel: unknown
  judgeProvider: unknown
  judgeModel: unknown
}) {
  const parts = [
    text(values.caseId),
    normalizedSeed(values.seed),
    typeof values.repetition === 'number' &&
    Number.isSafeInteger(values.repetition) &&
    values.repetition >= 0
      ? values.repetition
      : null,
    text(values.definitionSha256),
    text(values.resultContractSha256),
    text(values.scoringProfileSha256),
    text(values.subjectProvider),
    text(values.subjectModel),
    values.judgeProvider == null ? '' : text(values.judgeProvider),
    values.judgeModel == null ? '' : text(values.judgeModel),
  ]
  return parts.every((part) => part !== null) ? JSON.stringify(parts) : null
}

function primaryValues(runs: RcRun[]): PrimaryTestValues['values'] {
  const values = Object.fromEntries(
    (
      [
        'score',
        'totalTokens',
        'inputTokens',
        'outputTokens',
        'inputNormal',
        'cacheRead',
        'cacheWrite',
        'turns',
        'functionCalls',
        'functionErrors',
        'durationMs',
        'costUsd',
      ] satisfies MetricId[]
    ).map((id) => [id, []]),
  ) as unknown as PrimaryTestValues['values']
  for (const run of runs) {
    const complete = run.attemptsComplete
    values.score.push(
      typeof run.objectiveScore === 'number' &&
        Number.isFinite(run.objectiveScore) &&
        run.objectiveScore >= 0 &&
        run.objectiveScore <= 100
        ? run.objectiveScore
        : null,
    )
    values.totalTokens.push(complete ? run.totalTokens : null)
    values.inputTokens.push(complete ? run.record?.efficiency?.input_tokens ?? null : null)
    values.outputTokens.push(complete ? run.record?.efficiency?.output_tokens ?? null : null)
    values.inputNormal.push(null)
    values.cacheRead.push(complete ? run.record?.efficiency?.cache_read_tokens ?? null : null)
    values.cacheWrite.push(complete ? run.record?.efficiency?.cache_write_tokens ?? null : null)
    values.turns.push(complete ? run.turns : null)
    values.functionCalls.push(complete ? run.functionCalls : null)
    values.functionErrors.push(complete ? run.record?.efficiency?.function_call_errors ?? run.functionCallErrors ?? null : null)
    values.durationMs.push(run.wallTimeMs)
    // RC only retains subject cost; costUsd is total recorded spend.
    values.costUsd.push(complete ? run.record?.cost?.total_usd ?? run.costSubjectUsd : null)
  }
  return values
}

function plannedRepetitions(reference: RcReference) {
  const repetitions = record(
    record(reference.materialized)?.profile,
  )?.repetitions
  return typeof repetitions === 'number' &&
    Number.isSafeInteger(repetitions) &&
    repetitions > 0
    ? repetitions
    : null
}

/** Project the RC ledger directly into the shared presentation contract. */
export function referencePrimaryMetrics(
  reference: RcReference,
): PrimaryMetrics {
  const groups = new Map<string, RcRun[]>()
  for (const run of reference.runs) {
    const key = JSON.stringify([run.scenarioId, run.scenarioVersion])
    groups.set(key, [...(groups.get(key) ?? []), run])
  }
  const planned = plannedRepetitions(reference)
  const executionComplete =
    reference.aggregate.planned_runs === reference.aggregate.observed_runs
  const planSubject = record(record(reference.execution.plan)?.subject)
  const planJudge = record(record(reference.execution.plan)?.judge)
  return buildPrimaryMetricsFromValues(
    [...groups.entries()].map(([key, runs]) => {
      const identities = runs.map((run) =>
        primaryIdentity({
          caseId: run.caseId,
          seed: run.seed,
          repetition: run.repetition,
          definitionSha256: run.identity?.definitionSha256,
          resultContractSha256: run.identity?.resultContractSha256,
          scoringProfileSha256: run.identity?.scoringProfileSha256,
          subjectProvider:
            run.identity?.subjectProvider ?? planSubject?.provider,
          subjectModel: run.identity?.subjectModel ?? planSubject?.model,
          judgeProvider: run.identity?.judgeProvider ?? planJudge?.provider,
          judgeModel: run.identity?.judgeModel ?? planJudge?.model,
        }),
      )
      const repetitions = runs.map((run) => run.repetition)
      const expected = planned
      const scopeKnown =
        executionComplete &&
        expected !== null &&
        runs.length === expected &&
        repetitions.every(
          (value): value is number =>
            typeof value === 'number' &&
            Number.isSafeInteger(value) &&
            value >= 0 &&
            value < expected,
        ) &&
        new Set(repetitions).size === repetitions.length &&
        identities.every((identity) => identity !== null) &&
        new Set(identities).size === identities.length
      return {
        key,
        label: runs[0]?.scenarioId ?? key,
        version: runs[0]?.scenarioVersion ?? null,
        expected: expected ?? runs.length,
        scopeKnown,
        identity: scopeKnown ? JSON.stringify([...identities].sort()) : null,
        values: primaryValues(runs),
      }
    }),
  )
}

/** Keep local Results native while aligning only their comparison identity to RC. */
export function localReferencePrimaryMetrics(
  detail: DashboardExecutionDetail,
  compatible: boolean,
): PrimaryMetrics {
  const projected = buildPrimaryMetrics(detail)
  return {
    ...projected,
    tests: projected.tests.map((test) => {
      const identities = detail.reports.flatMap((entry) => {
        const subject = detail.subjects.find(
          (candidate) => candidate.id === entry.subject_id,
        )
        const reportSubject = record(entry.report?.subject)
        const reportJudge = record(entry.report?.judge)
        const judge = record(subject?.judge)
        return (
          entry.report?.scenarios.flatMap((scenario) => {
            if (
              scenario.scenario_id !== test.label ||
              scenario.scenario_version !== test.version
            )
              return []
            const scenarioCase = record(scenario.case)
            return scenario.runs.map((run, index) =>
              primaryIdentity({
                caseId: scenario.case_id,
                seed: scenarioCase?.seed,
                repetition: localRepetition(entry, run, index),
                definitionSha256: scenarioCase?.inputs_sha256,
                resultContractSha256: entry.report?.result_contract_sha256,
                scoringProfileSha256: entry.report?.scoring_profile_sha256,
                subjectProvider: reportSubject?.provider ?? subject?.provider,
                subjectModel: reportSubject?.model ?? subject?.model,
                judgeProvider: reportJudge?.provider ?? judge?.provider,
                judgeModel: reportJudge?.model ?? judge?.model,
              }),
            )
          }) ?? []
        )
      })
      const slotsKnown =
        identities.length === test.repetitions &&
        identities.every((identity) => identity !== null) &&
        new Set(identities).size === identities.length
      return {
        ...test,
        scopeKnown: test.scopeKnown && slotsKnown,
        identity:
          compatible && slotsKnown
            ? JSON.stringify([...identities].sort())
            : null,
      }
    }),
  }
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

function median(values: Array<number | null | undefined>) {
  const measured = values
    .filter(
      (value): value is number =>
        typeof value === 'number' && Number.isFinite(value),
    )
    .sort((left, right) => left - right)
  if (measured.length === 0) return null
  return (
    (measured[Math.floor((measured.length - 1) / 2)] +
      measured[Math.floor(measured.length / 2)]) /
    2
  )
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function text(value: unknown) {
  return typeof value === 'string' && value.length > 0 ? value : null
}

function seedNumber(value: string | number | null | undefined) {
  if (value === null || value === undefined || value === '') return null
  const number = Number(value)
  return Number.isSafeInteger(number) && number >= 0 ? number : null
}

function localRepetition(
  entry: Record<string, unknown>,
  run: DashboardRunProjection,
  index: number,
) {
  if (
    typeof entry.round === 'number' &&
    Number.isInteger(entry.round) &&
    entry.round > 0
  )
    return entry.round - 1
  return typeof run.repetition === 'number' &&
    Number.isInteger(run.repetition) &&
    run.repetition >= 0
    ? run.repetition
    : index
}

function aggregateStatus(runs: RcRun[]) {
  if (runs.length === 0) return 'unavailable'
  if (runs.every((run) => run.status === 'passed')) return 'passed'
  if (runs.some((run) => run.technical === 'technical_invalid'))
    return 'technical_failed'
  return (
    runs.find((run) => run.status && run.status !== 'passed')?.status ??
    'incomplete'
  )
}

function localRuns(detail: DashboardExecutionDetail): RcRun[] {
  const seen = new Set<string>()
  return detail.reports.flatMap(
    (entry, entryIndex) =>
      entry.report?.scenarios.flatMap((scenario, scenarioIndex) => {
        const scenarioCase = record(scenario.case)
        const caseId = text(scenario.case_id)
        const seed = scenarioCase?.seed
        return scenario.runs.flatMap((run, runIndex) => {
          const executionId =
            entry.native_execution_id ?? `report:${entryIndex}`
          const id = `${executionId}:${scenarioIndex}:${run.run_id}`
          if (seen.has(id)) return []
          seen.add(id)
          const attemptsComplete = localAttemptsComplete(run)
          return [
            {
              id,
              attemptsComplete,
              scenarioId: scenario.scenario_id,
              scenarioVersion: scenario.scenario_version,
              caseId,
              seed:
                typeof seed === 'string' || typeof seed === 'number'
                  ? String(seed)
                  : null,
              repetition: localRepetition(entry, run, runIndex),
              status: run.status,
              completion: run.completion,
              technical: run.technical,
              objectiveScore: run.objective_score,
              wallTimeMs: run.wall_time_ms ?? null,
              totalTokens: inclusiveTokens(run),
              costSubjectUsd: attemptsComplete
                ? (run.cost?.subject_usd ?? null)
                : null,
              turns:
                attemptsComplete &&
                typeof run.efficiency?.root_turns === 'number'
                  ? run.efficiency.root_turns +
                    (typeof run.efficiency.child_turns === 'number'
                      ? run.efficiency.child_turns
                      : 0)
                  : null,
              functionCalls: attemptsComplete
                ? (run.efficiency?.function_calls ?? null)
                : null,
              functionCallErrors: attemptsComplete
                ? (run.efficiency?.function_call_errors ?? null)
                : null,
            },
          ]
        })
      }) ?? [],
  )
}

function observation(
  runs: RcRun[],
  context: {
    observationId: string
    executionId: string
    completedAt: string
    source: TestObservation['source']
    sourceUrl: string | null
    subjectProvider: string
    subjectModel: string
    judgeProvider: string | null
    judgeModel: string | null
    contractSha256?: string
  },
): TestObservation {
  const complete = runs.map((run) =>
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
  const scores = complete.map((run) => run.objectiveScore)
  const cohorts = new Set(
    complete.map((run) => run.cohortSha256).filter(Boolean),
  )
  const harnesses = new Set(
    complete.map((run) => run.identity?.harnessVersion).filter(Boolean),
  )
  return {
    observation_id: context.observationId,
    source: context.source,
    source_url: context.sourceUrl,
    execution_id: context.executionId,
    evaluated_version_id: null,
    cohort_id: cohorts.size === 1 ? ([...cohorts][0] ?? '') : '',
    completed_at: context.completedAt,
    case_id: complete[0]?.caseId ?? '',
    contract_sha256: context.contractSha256 ?? '',
    assessment_profile_sha256: '',
    status: aggregateStatus(complete),
    median_score: median(scores),
    run_count: complete.length,
    scored_runs: scores.filter(
      (score) => typeof score === 'number' && Number.isFinite(score),
    ).length,
    scenario_version: complete[0]?.scenarioVersion ?? undefined,
    seed: seedNumber(complete[0]?.seed),
    system_version_id: null,
    system_label:
      harnesses.size === 1 ? `Harness ${[...harnesses][0]}` : 'Unknown system',
    stack_mode: '',
    harness_revision: null,
    system_revision: null,
    engine_revision: null,
    subject_provider: context.subjectProvider,
    subject_model: context.subjectModel,
    judge_provider: context.judgeProvider,
    judge_model: context.judgeModel,
    median_cost_usd: median(complete.map((run) => run.costSubjectUsd)),
    median_tokens: median(complete.map((run) => run.totalTokens)),
    median_duration_seconds: median(
      complete.map((run) =>
        run.wallTimeMs === null ? null : run.wallTimeMs / 1000,
      ),
    ),
    median_function_calls: median(complete.map((run) => run.functionCalls)),
    median_function_call_errors: median(
      complete.map((run) => run.functionCallErrors),
    ),
    median_turns: median(complete.map((run) => run.turns)),
  }
}

/** Project one RC execution into per-case observations for a single test. */
export function referenceScenarioObservations(
  reference: RcReference,
  scenarioId: string,
): TestObservation[] {
  const groups = new Map<string, RcRun[]>()
  reference.runs
    .filter((run) => run.scenarioId === scenarioId)
    .forEach((run, index) => {
      const identified =
        run.caseId != null && run.scenarioVersion != null && run.seed != null
      const key = identified
        ? JSON.stringify([run.caseId, run.scenarioVersion, run.seed])
        : `unknown:${run.id ?? index}`
      groups.set(key, [...(groups.get(key) ?? []), run])
    })
  const plan = record(reference.execution.plan)
  const subject = record(plan?.subject)
  const judge = record(plan?.judge)
  return [...groups.entries()].map(([observationId, runs]) =>
    observation(runs, {
      observationId,
      executionId: reference.execution.local_id ?? reference.execution.id,
      completedAt:
        reference.execution.completedAt ?? reference.execution.requestedAt,
      source: 'release-control',
      sourceUrl: reference.execution.ghRunUrl ?? null,
      subjectProvider:
        text(subject?.provider) ??
        text(runs[0]?.identity?.subjectProvider) ??
        '',
      subjectModel:
        text(subject?.model) ?? text(runs[0]?.identity?.subjectModel) ?? '',
      judgeProvider:
        text(judge?.provider) ?? text(runs[0]?.identity?.judgeProvider),
      judgeModel: text(judge?.model) ?? text(runs[0]?.identity?.judgeModel),
    }),
  )
}

/** Project local Results with the same score, token and subject-cost semantics as RC. */
export function localScenarioObservations(
  detail: DashboardExecutionDetail,
  scenarioId: string,
): TestObservation[] {
  const groups = new Map<string, RcRun[]>()
  for (const run of localRuns(detail).filter(
    (candidate) => candidate.scenarioId === scenarioId,
  )) {
    const identified =
      run.caseId != null && run.scenarioVersion != null && run.seed != null
    const identity = identified
      ? JSON.stringify([run.caseId, run.scenarioVersion, run.seed])
      : `unknown:${run.id}`
    groups.set(identity, [...(groups.get(identity) ?? []), run])
  }
  const subject = detail.subjects[0]
  const judge = record(subject?.judge)
  const metric = detail.scenario_metrics?.find(
    (candidate) => candidate.scenario_id === scenarioId,
  )
  return [...groups.entries()].map(([observationId, runs]) =>
    observation(runs, {
      observationId,
      executionId: detail.id,
      completedAt: detail.completed_at ?? detail.started_at ?? '',
      source: 'local',
      sourceUrl: null,
      subjectProvider: subject?.provider ?? '',
      subjectModel: subject?.model ?? '',
      judgeProvider: text(judge?.provider),
      judgeModel: text(judge?.model),
      contractSha256: text(metric?.contract_fingerprint) ?? '',
    }),
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

export async function listImportedExecutions(): Promise<
  DashboardExecutionSummary[]
> {
  const bridge = await getDashboardDataBridge()
  const executions: DashboardExecutionSummary[] = []
  let cursor: string | undefined
  do {
    const page = await bridge.listExecutions({ limit: 200, cursor })
    executions.push(...page.executions.filter((execution) => execution.origin === 'remote'))
    cursor = page.next_cursor ?? undefined
  } while (cursor)
  return executions
}

export async function getImportedReference(
  id: string,
): Promise<RcReference> {
  const detail = await (await getDashboardDataBridge()).getExecution(id)
  const reference = detail.remote_reference
  if (!reference || typeof reference !== 'object')
    throw new Error('Imported execution does not retain a reference ledger.')
  return reference as RcReference
}
