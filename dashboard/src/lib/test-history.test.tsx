import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { LossesPanel, TrendChart } from '@/components/history/TrendChart'
import {
  axisTop,
  definitionChoices,
  differenceText,
  losses,
  metricValue,
  niceMax,
  scoreStatus,
  staleNotice,
  summaryFigures,
  toggleSelection,
} from '@/lib/test-history'
import {
  formFlow,
  formFlowRow,
  history,
  run,
} from '@/test-fixtures/test-history'

const C = `sha256:${'c3'.repeat(32)}`

describe('definitions', () => {
  it('lists the current definition first, then the most recently run', () => {
    expect(
      definitionChoices(formFlow).map((choice) => [
        choice.label,
        choice.runs,
        choice.current,
      ]),
    ).toEqual([
      ['1768da69', 1, true],
      ['0c5c0902', 4, false],
      ['6831b628', 1, false],
    ])
  })

  it('says when the current definition has not run yet', () => {
    expect(staleNotice(formFlow)).toBeNull()
    const moved = history({
      current_version: C,
      available_versions: [
        ...formFlow.available_versions,
        { version: C, execution_count: 0, run_count: 0, last_seen: null },
      ],
    })
    expect(definitionChoices(moved)[0]).toMatchObject({
      label: 'c3c3c3c3',
      runs: 0,
      current: true,
    })
    expect(staleNotice(moved)).toBe(
      'The current definition c3c3c3c3 hasn’t run yet. The 6 runs below belong to 3 earlier definitions, so they don’t say how the test behaves now.',
    )
    // A test that never ran has nothing below to warn about.
    expect(
      staleNotice(
        history({
          current_version: C,
          available_versions: [
            { version: C, execution_count: 0, run_count: 0, last_seen: null },
          ],
        }),
      ),
    ).toBeNull()
  })
})

describe('the chart and the summary', () => {
  it('scales the axis to a round top over the peak', () => {
    expect(niceMax(170_900)).toBe(200_000)
    expect(niceMax(196)).toBe(200)
    expect(niceMax(72)).toBe(75)
    expect(axisTop('score', 40)).toBe(100)
    // Durations past two minutes scale in whole minutes: 37m 48s → 40m.
    expect(axisTop('duration', 2_268_000)).toBe(2_400_000)
    expect(axisTop('duration', 90_000)).toBe(90_000)
  })

  it('reads a zero duration, token count or turn count as not reported', () => {
    const quiet = { ...run('9:13'), median_tokens: 0, mean_score: 0 }
    expect(metricValue(quiet, 'tokens')).toBeNull()
    expect(metricValue(quiet, 'score')).toBe(0)
    expect(metricValue(run('9:13'), 'duration')).toBe(2_268_000)
  })

  it('sums up the runs in scope as the canvas does', () => {
    expect(
      summaryFigures(formFlow.observations).map((figure) => [
        figure.label,
        figure.value,
        figure.sub,
      ]),
    ).toEqual([
      ['Mean score', '79', 'over 6 scored runs'],
      ['Full marks', '3/6', '3 lost points'],
      ['Runs', '6', 'all scored'],
      ['Median duration', '9m 26s', 'per run'],
      ['Median tokens', '58.3K', 'input + output, per scored run'],
    ])
  })

  it('ranks the criteria lost across the scored runs', () => {
    const ranked = losses(formFlow.observations, formFlowRow.spec ?? null)
    expect(ranked[0]).toEqual({
      id: 'browser_interaction',
      share: 33,
      text: '2 of 6 · 10 pts',
    })
    expect(ranked.map((loss) => loss.id)).toContain('runtime_contract')
    expect(ranked).toHaveLength(6)
  })

  it('draws a mark per run, the definition changes and the legend', () => {
    const html = renderToStaticMarkup(
      <TrendChart
        observations={formFlow.observations}
        metric="score"
        onMetric={() => undefined}
      />,
    )
    expect(html).toContain('data-history-chart="score"')
    expect((html.match(/<circle/g) ?? []).length).toBe(6)
    // 6831b628 → 0c5c0902 → 1768da69, oldest first.
    expect(html).toContain('>0c5c0902</text>')
    expect(html).toContain('>1768da69</text>')
    expect(html).toContain('deepseek-flash')
    expect(html).toContain('claude-opus-5-5')
    expect(html).not.toContain('Model turns per run.')
    expect(
      renderToStaticMarkup(
        <TrendChart
          observations={formFlow.observations}
          metric="turns"
          onMetric={() => undefined}
        />,
      ),
    ).toContain('Model turns per run.')
    expect(renderToStaticMarkup(<LossesPanel losses={[]} />)).toContain(
      'Every scored run met every criterion.',
    )
  })
})

describe('a run', () => {
  it('reads its score as the canvas does', () => {
    expect(scoreStatus(run('9:13'))).toBe('lost 10')
    expect(scoreStatus(run('7:50'))).toBe('no points')
    expect(scoreStatus(run('3:29'))).toBe('full marks')
    const limit = run('3:29')
    limit.mean_score = null
    if (limit.runs) {
      limit.runs[0].status = 'resource_limit'
      limit.runs[0].score = null
    }
    expect(scoreStatus(limit)).toBe('hit a limit')
  })

  it('ticks A first, then B, and never a third', () => {
    expect(toggleSelection([], 'a')).toEqual(['a'])
    expect(toggleSelection(['a'], 'b')).toEqual(['a', 'b'])
    expect(toggleSelection(['a', 'b'], 'c')).toEqual(['a', 'b'])
    expect(toggleSelection(['a', 'b'], 'a')).toEqual(['b'])
  })

  it('writes B minus A without calling either better', () => {
    const count = (value: number) => String(value)
    expect(differenceText(90, 100, count, 'points')).toBe('+10 pts')
    expect(differenceText(196, 53, count)).toBe('−143 · −73%')
    expect(differenceText(4, 4, count)).toBe('no change')
    expect(differenceText(null, 4, count)).toBe('—')
  })
})
