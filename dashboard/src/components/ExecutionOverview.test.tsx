import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  completionIsTrivial,
  ExecutionOverview,
  p50EqualsPerCompletion,
  usageCoverageComplete,
} from '@/components/ExecutionOverview'
import { buildExecutionMetrics } from '@/lib/execution-metrics'
import {
  executionMetricsFixture,
  metricRun,
} from '@/test-fixtures/execution-metrics'

const boundaries = [
  { role: 'system' as const, value: 'passed' },
  { role: 'advisory' as const, value: 'pass' },
]

describe('execution overview', () => {
  // Audit ED-27: three ratios that are 100% by construction are one fact.
  it('folds the three completion ratios into one tile when nothing went wrong', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 4_000, { quality_score_completed: 90 })] },
      { runs: [metricRun('b', 3_781, { quality_score_completed: 100 })] },
    ])
    const metrics = buildExecutionMetrics(detail)
    expect(completionIsTrivial(metrics)).toBe(true)
    const html = renderToStaticMarkup(
      <ExecutionOverview detail={detail} boundaries={boundaries} />,
    )
    expect(html).toContain('data-completion="trivial"')
    expect(html).toContain('every planned run completed')
    expect(html).toContain('2/2')
    expect(html).not.toContain('completion rate')
    expect(html).not.toContain('execution reliability')
    // The derivation leads, in the same shape the evidence record uses.
    expect(html).toContain('data-outcome-derivation')
  })

  it('keeps the three ratios apart as soon as one run did not complete', () => {
    const detail = executionMetricsFixture([
      { runs: [metricRun('a', 100_000, { quality_score_completed: 60 })] },
      {
        runs: [
          metricRun('b', 20_000, {
            completion: 'task_incomplete',
            quality_score_completed: null,
          }),
        ],
      },
    ])
    const metrics = buildExecutionMetrics(detail)
    expect(completionIsTrivial(metrics)).toBe(false)
    const html = renderToStaticMarkup(
      <ExecutionOverview detail={detail} boundaries={boundaries} />,
    )
    expect(html).toContain('data-completion="detailed"')
    expect(html).toContain('completion rate')
    expect(html).toContain('execution reliability')
    expect(html).toContain('completion evidence')
    expect(html).not.toContain('every planned run completed')
  })

  // Audit ED-28: "N/N runs with telemetry" once in the band, not per card.
  it('states complete coverage once and per metric only when it is partial', () => {
    const complete = executionMetricsFixture([
      { runs: [metricRun('a', 4_000)] },
      { runs: [metricRun('b', 3_781)] },
    ])
    expect(usageCoverageComplete(buildExecutionMetrics(complete))).toBe(true)
    const full = renderToStaticMarkup(
      <ExecutionOverview detail={complete} boundaries={boundaries} />,
    )
    expect(full).toContain('data-usage-coverage="complete"')
    expect(full).toContain('subject telemetry from 2 of 2 runs')
    expect(full).not.toContain('runs with telemetry')

    const partial = executionMetricsFixture([
      {
        runs: [
          metricRun('a', 100, { cost: null }),
          metricRun('b', null, { cost: null }),
        ],
      },
    ])
    expect(usageCoverageComplete(buildExecutionMetrics(partial))).toBe(false)
    const html = renderToStaticMarkup(
      <ExecutionOverview detail={partial} boundaries={boundaries} />,
    )
    expect(html).toContain('data-usage-coverage="partial"')
    expect(html).toContain('1/2 runs with telemetry')
    expect(html).toContain('observed subtotal')
  })

  // The median of two values is their mean: one card, with the note.
  it('shows completed p50 as its own card only when it can differ', () => {
    const two = buildExecutionMetrics(
      executionMetricsFixture([
        { runs: [metricRun('a', 4_000)] },
        { runs: [metricRun('b', 3_781)] },
      ]),
    )
    expect(p50EqualsPerCompletion(two)).toBe(true)
    const three = executionMetricsFixture([
      { runs: [metricRun('a', 4_000)] },
      { runs: [metricRun('b', 3_781)] },
      { runs: [metricRun('c', 9_000)] },
    ])
    expect(p50EqualsPerCompletion(buildExecutionMetrics(three))).toBe(false)
    const html = renderToStaticMarkup(
      <ExecutionOverview detail={three} boundaries={boundaries} />,
    )
    expect(html).toContain('completed p50 tokens')
  })

  it('says when there is nothing to consolidate instead of inventing zeros', () => {
    const detail = executionMetricsFixture([{ runs: [metricRun('a', 100)] }])
    detail.reports[0].available = false
    const html = renderToStaticMarkup(
      <ExecutionOverview detail={detail} boundaries={boundaries} />,
    )
    expect(html).toContain('No compatible run evidence')
    expect(html).not.toContain('100%')
  })
})
