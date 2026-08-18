import type { OperationalStatus } from '@/design-system'
import type {
  DashboardExecutionDetail,
  DashboardReportProjection,
  DashboardRunProjection,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import {
  aggregateWorkflowMetrics,
  generalRunMetrics,
  workflowStepUsage,
} from '@/lib/workflow-metrics'

type ScenarioProjection = DashboardReportProjection['scenarios'][number]

export type ScenarioMatrixItem = {
  key: string
  reportIndex: number
  scenarioIndex: number | null
  subjectId: string
  scenarioId: string
  scenarioVersion: number | null
  available: boolean
  objective: {
    status: OperationalStatus
    label: string
    raw: string
  }
  advisory: {
    status: OperationalStatus
    label: string
  }
  durationMs: number | null
  durationKind: 'single' | 'average' | null
  runCount: number
  runs: DashboardRunProjection[]
  primaryRun: DashboardRunProjection | null
  workflowRun: DashboardRunProjection | null
  workflowSteps: SemanticTestReport[]
  primaryMetrics: Array<{ label: string; value: string; detail: string }>
}

export type ScenarioMatrixSummary = {
  total: number
  passed: number
  failed: number
  hardGate: number
  inconclusive: number
  unavailable: number
  running: number
  incomplete: number
}

export type ScenarioMatrixModel = {
  items: ScenarioMatrixItem[]
  summary: ScenarioMatrixSummary
}

export function buildScenarioMatrix(
  detail: DashboardExecutionDetail,
): ScenarioMatrixModel {
  const items = (detail.reports ?? []).flatMap((record, reportIndex) => {
    if (!record.available || !record.report) {
      return [unavailableScenario(detail, reportIndex)]
    }

    return (record.report.scenarios ?? []).map((scenario, scenarioIndex) =>
      scenarioItem(
        detail,
        record.subject_id,
        scenario,
        reportIndex,
        scenarioIndex,
      ),
    )
  })

  const summary: ScenarioMatrixSummary = {
    total: items.length,
    passed: 0,
    failed: 0,
    hardGate: 0,
    inconclusive: 0,
    unavailable: 0,
    running: 0,
    incomplete: 0,
  }
  for (const item of items) {
    if (item.objective.status === 'passed') summary.passed += 1
    else if (item.objective.status === 'hard_gate') summary.hardGate += 1
    else if (item.objective.status === 'inconclusive') summary.inconclusive += 1
    else if (item.objective.status === 'unavailable') summary.unavailable += 1
    else if (item.objective.status === 'running') summary.running += 1
    else if (
      item.objective.status === 'incomplete' ||
      item.objective.status === 'cancelled' ||
      item.objective.status === 'cancelling'
    )
      summary.incomplete += 1
    else summary.failed += 1
  }

  return { items, summary }
}

export function detailForScenario(
  detail: DashboardExecutionDetail,
  item: ScenarioMatrixItem,
): DashboardExecutionDetail {
  const record = detail.reports[item.reportIndex]
  if (!record) return { ...detail, reports: [] }
  if (!record.report || item.scenarioIndex == null) {
    return { ...detail, reports: [{ ...record }] }
  }
  const scenario = record.report.scenarios[item.scenarioIndex]
  return {
    ...detail,
    reports: [
      {
        ...record,
        report: {
          ...record.report,
          scenarios: scenario ? [scenario] : [],
        },
      },
    ],
  }
}

function scenarioItem(
  detail: DashboardExecutionDetail,
  subjectId: string,
  scenario: ScenarioProjection,
  reportIndex: number,
  scenarioIndex: number,
): ScenarioMatrixItem {
  const runs = scenario.runs ?? []
  const primaryRun = runs.at(-1) ?? null
  const workflowRun = [...runs]
    .reverse()
    .find((run) => (run.semantic_tests?.length ?? 0) > 0)
  const workflowSteps = workflowRun?.semantic_tests ?? []
  const objective = objectiveStatus(
    scenario.status ||
      primaryRun?.assessment?.system_status ||
      primaryRun?.status ||
      (scenario.passed === true
        ? 'passed'
        : scenario.passed === false
          ? 'failed'
          : 'unavailable'),
  )
  const duration = scenarioDuration(detail, subjectId, scenario, runs)

  return {
    key: `${subjectId}:${scenario.scenario_id}:v${scenario.scenario_version}:${reportIndex}:${scenarioIndex}`,
    reportIndex,
    scenarioIndex,
    subjectId,
    scenarioId: scenario.scenario_id,
    scenarioVersion: scenario.scenario_version,
    available: true,
    objective,
    advisory: advisoryStatus(primaryRun),
    durationMs: duration.value,
    durationKind: duration.kind,
    runCount: runs.length,
    runs,
    primaryRun,
    workflowRun: workflowRun ?? null,
    workflowSteps,
    primaryMetrics: primaryMetrics(primaryRun, workflowSteps, duration),
  }
}

function unavailableScenario(
  detail: DashboardExecutionDetail,
  reportIndex: number,
): ScenarioMatrixItem {
  const record = detail.reports[reportIndex]
  const scenarioId = record?.scenario_id || 'unknown_scenario'
  const summary = detail.subjects
    .find((subject) => subject.id === record?.subject_id)
    ?.scenarios.find((scenario) => scenario.id === scenarioId)

  return {
    key: `${record?.subject_id ?? 'unknown'}:${scenarioId}:unavailable:${reportIndex}`,
    reportIndex,
    scenarioIndex: null,
    subjectId: record?.subject_id ?? 'Unknown subject',
    scenarioId,
    scenarioVersion: summary?.scenario_version ?? null,
    available: false,
    objective: {
      status: 'unavailable',
      label: 'Unavailable',
      raw: 'unavailable',
    },
    advisory: { status: 'unavailable', label: 'No report' },
    durationMs: null,
    durationKind: null,
    runCount: 0,
    runs: [],
    primaryRun: null,
    workflowRun: null,
    workflowSteps: [],
    primaryMetrics: primaryMetrics(null, [], { value: null, kind: null }),
  }
}

function objectiveStatus(rawValue: string): ScenarioMatrixItem['objective'] {
  const raw = rawValue.toLowerCase()
  if (raw === 'passed' || raw === 'pass' || raw === 'succeeded') {
    return { status: 'passed', label: 'Passed', raw }
  }
  if (raw === 'hard_gate_failed') {
    return { status: 'hard_gate', label: 'Hard gate failed', raw }
  }
  if (raw === 'inconclusive') {
    return { status: 'inconclusive', label: 'Inconclusive', raw }
  }
  if (raw === 'unavailable' || raw === 'not_evaluated') {
    return { status: 'unavailable', label: 'Unavailable', raw }
  }
  if (raw === 'running') return { status: 'running', label: 'Running', raw }
  if (raw === 'cancelling') {
    return { status: 'cancelling', label: 'Cancelling', raw }
  }
  if (raw === 'cancelled') {
    return { status: 'cancelled', label: 'Cancelled', raw }
  }
  if (raw === 'incomplete' || raw === 'pending' || raw === 'skipped') {
    return { status: 'incomplete', label: humanize(raw), raw }
  }
  return { status: 'failed', label: humanize(raw || 'failed'), raw }
}

function advisoryStatus(
  run: DashboardRunProjection | null,
): ScenarioMatrixItem['advisory'] {
  const assessment = run?.assessment?.ai_final_assessment
  const verdict = assessment?.result?.verdict
  if (verdict === 'pass') return { status: 'passed', label: 'AI passed' }
  if (verdict === 'pass_with_concerns') {
    return { status: 'recommendation', label: 'AI concerns' }
  }
  if (verdict === 'fail') return { status: 'failed', label: 'AI failed' }
  if (verdict === 'inconclusive') {
    return { status: 'inconclusive', label: 'AI inconclusive' }
  }
  if (assessment?.availability === 'failed') {
    return { status: 'failed', label: 'AI unavailable' }
  }
  return { status: 'unavailable', label: 'No advisory' }
}

function scenarioDuration(
  detail: DashboardExecutionDetail,
  subjectId: string,
  scenario: ScenarioProjection,
  runs: DashboardRunProjection[],
): { value: number | null; kind: 'single' | 'average' | null } {
  const metric = detail.scenario_metrics?.find(
    (candidate) =>
      candidate.scenario_id === scenario.scenario_id &&
      (!candidate.subject_id || candidate.subject_id === subjectId),
  )
  const averageSeconds = finiteNumber(metric?.averages?.duration_seconds)
  if (averageSeconds != null) {
    return { value: averageSeconds * 1000, kind: 'average' }
  }
  const values = runs
    .map((run) =>
      finiteNumber(run.wall_time_ms ?? run.efficiency?.wall_time_ms),
    )
    .filter((value): value is number => value != null)
  if (values.length === 0) return { value: null, kind: null }
  if (values.length === 1) return { value: values[0], kind: 'single' }
  return {
    value: values.reduce((total, value) => total + value, 0) / values.length,
    kind: 'average',
  }
}

function primaryMetrics(
  run: DashboardRunProjection | null,
  tests: SemanticTestReport[],
  duration: { value: number | null; kind: 'single' | 'average' | null },
): ScenarioMatrixItem['primaryMetrics'] {
  const runUsage = generalRunMetrics(run, tests)
  const workflow = tests.length > 0 ? aggregateWorkflowMetrics(tests) : null
  const workflowCosts = tests.map((test) => finiteNumber(test.cost_usd))
  const workflowCost =
    workflowCosts.length > 0 &&
    workflowCosts.every((value): value is number => value != null)
      ? workflowCosts.reduce((total, value) => total + value, 0)
      : null
  const totalTokens =
    runUsage.totalTokens ??
    (workflow && workflow.tokenMetricSteps > 0 ? workflow.totalTokens : null)
  const functionCalls =
    runUsage.functionCalls ??
    (workflow && workflow.functionCallMetricSteps > 0
      ? workflow.functionCalls
      : null)
  const functionErrors =
    runUsage.functionCallErrors ??
    (workflow && workflow.functionCallErrorMetricSteps > 0
      ? workflow.functionCallErrors
      : null)
  const costUsd = runUsage.costUsd ?? workflowCost
  const durationMs =
    duration.value ??
    finiteNumber(run?.wall_time_ms ?? run?.efficiency?.wall_time_ms)
  const missingDetail = run ? 'Not captured for this run' : 'No run retained'
  const runtimeMetric = {
    label: 'Runtime',
    value: durationMs == null ? '—' : formatDuration(durationMs),
    detail:
      duration.kind === 'average'
        ? 'Average scenario duration'
        : durationMs == null
          ? missingDetail
          : 'Subject execution time',
  }
  return [
    runtimeMetric,
    {
      label: 'Total tokens',
      value: totalTokens == null ? '—' : formatCount(totalTokens),
      detail:
        runUsage.totalTokens != null
          ? runUsage.backfilled
            ? 'Evaluator usage · backfilled'
            : 'Subject execution usage'
          : workflow && workflow.tokenMetricSteps > 0
            ? `${workflow.tokenMetricSteps}/${workflow.stepCount} workflow steps reported`
            : missingDetail,
    },
    {
      label: 'Function calls',
      value: functionCalls == null ? '—' : formatCount(functionCalls),
      detail:
        runUsage.backfilled && functionCalls != null
          ? 'Workflow operations · backfilled'
          : functionErrors == null
            ? functionCalls == null
              ? missingDetail
              : 'Error count not captured'
            : `${formatCount(functionErrors)} errors`,
    },
    {
      label: 'Function errors',
      value: functionErrors == null ? '—' : formatCount(functionErrors),
      detail:
        functionErrors == null
          ? missingDetail
          : runUsage.backfilled
            ? 'Workflow failures · backfilled'
            : 'Subject execution errors',
    },
    {
      label: 'Reported cost',
      value: costUsd == null ? '—' : formatCost(costUsd),
      detail:
        runUsage.costUsd != null
          ? runUsage.backfilled
            ? 'Local run · no metered charge'
            : 'Subject execution cost'
          : workflowCost != null && workflow
            ? `Reported by all ${workflow.stepCount} workflow steps`
            : missingDetail,
    },
  ]
}

export type StepMetricSignal = {
  label: string
  value: string
}

export function stepSignals(test: SemanticTestReport): StepMetricSignal[] {
  const signals: StepMetricSignal[] = []
  const usage = workflowStepUsage(test.metrics)
  if (usage.totalTokens != null) {
    signals.push({ label: 'Tokens', value: formatCount(usage.totalTokens) })
  } else {
    if (usage.inputTokens != null) {
      signals.push({
        label: 'Input tokens',
        value: formatCount(usage.inputTokens),
      })
    }
    if (usage.outputTokens != null) {
      signals.push({
        label: 'Output tokens',
        value: formatCount(usage.outputTokens),
      })
    }
  }
  if (usage.functionCalls != null) {
    signals.push({
      label: 'Function calls',
      value: formatCount(usage.functionCalls),
    })
  }
  if (usage.functionCallErrors != null) {
    signals.push({
      label: 'Function errors',
      value: formatCount(usage.functionCallErrors),
    })
  }
  const cost = finiteNumber(test.cost_usd)
  if (cost != null) {
    signals.push({ label: 'Cost', value: formatCost(cost) })
  }
  return signals.length > 0
    ? signals
    : [{ label: 'Metrics', value: 'Not captured' }]
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? value
    : null
}

export function formatScenarioDuration(milliseconds: number | null) {
  if (milliseconds == null) return '—'
  return formatDuration(milliseconds)
}

function formatDuration(milliseconds: number) {
  if (milliseconds < 1_000) return `${Math.round(milliseconds)} ms`
  if (milliseconds < 60_000) {
    const seconds = milliseconds / 1_000
    return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`
  }
  const minutes = Math.floor(milliseconds / 60_000)
  const seconds = Math.round((milliseconds % 60_000) / 1_000)
  return `${minutes}m ${String(seconds).padStart(2, '0')}s`
}

function formatCount(value: number) {
  return Math.round(value).toLocaleString('en-US')
}

function formatCost(value: number) {
  if (value > 0 && value < 0.0001) return '<$0.0001'
  return `$${value.toFixed(4)}`
}

function humanize(value: string) {
  return value
    .replaceAll('.', ' / ')
    .replaceAll('_', ' ')
    .replaceAll('-', ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}
