import type { OperationalStatus } from '@/design-system'
import type {
  DashboardExecutionDetail,
  DashboardReportProjection,
  DashboardRunProjection,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import {
  aggregateWorkflowMetrics,
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
  hardGates: { passed: number; total: number }
  primarySignal: { label: string; value: string; detail: string }
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
  const hardGates = hardGateSummary(primaryRun, workflowSteps)

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
    hardGates,
    primarySignal: primarySignal(primaryRun, workflowSteps),
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
    hardGates: { passed: 0, total: 0 },
    primarySignal: {
      label: 'Evidence',
      value: 'Unavailable',
      detail: 'The expected scenario report was not retained.',
    },
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

function hardGateSummary(
  run: DashboardRunProjection | null,
  tests: SemanticTestReport[],
): { passed: number; total: number } {
  const stepGates = tests.flatMap((test) => test.hard_gates ?? [])
  if (stepGates.length > 0) {
    return {
      passed: stepGates.filter((gate) => gate.passed).length,
      total: stepGates.length,
    }
  }
  const assessments = run?.assessment?.assessments ?? []
  const gates = assessments.filter((entry) => entry.policy === 'hard_gate')
  return {
    passed: gates.filter((entry) => entry.outcome === 'passed').length,
    total: gates.length,
  }
}

function primarySignal(
  run: DashboardRunProjection | null,
  tests: SemanticTestReport[],
): ScenarioMatrixItem['primarySignal'] {
  const runUsage = runUsageSummary(run)
  if (runUsage.totalTokens != null) {
    return {
      label: 'Total tokens',
      value: formatCount(runUsage.totalTokens),
      detail: 'Subject execution usage',
    }
  }
  if (runUsage.functionCalls != null) {
    return {
      label: 'Function calls',
      value: formatCount(runUsage.functionCalls),
      detail:
        runUsage.functionCallErrors == null
          ? 'Error count not captured'
          : `${formatCount(runUsage.functionCallErrors)} errors`,
    }
  }
  if (runUsage.functionCallErrors != null) {
    return {
      label: 'Function errors',
      value: formatCount(runUsage.functionCallErrors),
      detail: 'Subject execution errors',
    }
  }
  if (runUsage.costUsd != null) {
    return {
      label: 'Reported cost',
      value: formatCost(runUsage.costUsd),
      detail: 'Subject execution cost',
    }
  }
  if (tests.length > 0) {
    const workflow = aggregateWorkflowMetrics(tests)
    if (workflow.tokenMetricSteps > 0) {
      return {
        label: 'Workflow tokens',
        value: formatCount(workflow.totalTokens),
        detail: `${workflow.tokenMetricSteps}/${workflow.stepCount} steps reported`,
      }
    }
    if (workflow.functionCallMetricSteps > 0) {
      return {
        label: 'Function calls',
        value: formatCount(workflow.functionCalls),
        detail:
          workflow.functionCallErrorMetricSteps > 0
            ? `${formatCount(workflow.functionCallErrors)} errors · ${workflow.functionCallMetricSteps}/${workflow.stepCount} steps reported`
            : `${workflow.functionCallMetricSteps}/${workflow.stepCount} steps reported`,
      }
    }
    if (workflow.functionCallErrorMetricSteps > 0) {
      return {
        label: 'Function errors',
        value: formatCount(workflow.functionCallErrors),
        detail: `${workflow.functionCallErrorMetricSteps}/${workflow.stepCount} steps reported`,
      }
    }
    const costs = tests.map((test) => finiteNumber(test.cost_usd))
    if (costs.every((value): value is number => value != null)) {
      return {
        label: 'Workflow cost',
        value: formatCost(costs.reduce((total, value) => total + value, 0)),
        detail: `Reported by all ${workflow.stepCount} steps`,
      }
    }
    return {
      label: 'Recorded time',
      value: formatDuration(workflow.durationMs),
      detail: `${workflow.stepCount} workflow steps`,
    }
  }
  if (runUsage.durationMs != null) {
    return {
      label: 'Runtime',
      value: formatDuration(runUsage.durationMs),
      detail: 'Subject execution time',
    }
  }
  return {
    label: 'Efficiency metrics',
    value: 'Not captured',
    detail: run ? 'No canonical telemetry retained' : 'No run retained',
  }
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

function runUsageSummary(run: DashboardRunProjection | null) {
  const totals = run?.metrics?.totals
  const input = finiteNumber(totals?.input_tokens)
  const output = finiteNumber(totals?.output_tokens)
  const explicitTotal = finiteNumber(
    totals?.total_tokens ?? run?.efficiency?.total_tokens,
  )
  return {
    totalTokens:
      explicitTotal ??
      (input != null && output != null ? input + output : null),
    functionCalls: finiteNumber(
      totals?.function_calls ?? run?.efficiency?.function_calls,
    ),
    functionCallErrors: finiteNumber(
      totals?.function_call_errors ?? run?.efficiency?.function_call_errors,
    ),
    costUsd: finiteNumber(run?.cost?.total_usd),
    durationMs: finiteNumber(
      run?.wall_time_ms ?? run?.efficiency?.wall_time_ms,
    ),
  }
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
