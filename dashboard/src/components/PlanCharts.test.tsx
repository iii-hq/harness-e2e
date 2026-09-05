import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { DivergingBars, Dumbbell, Sparkline } from '@/components/PlanCharts'

describe('plan charts', () => {
  it('draws one sparkline point per execution, the reference as a hairline', () => {
    const html = renderToStaticMarkup(
      <Sparkline
        label="Tokens across executions"
        reference={19_328}
        points={[
          {
            id: 'b',
            label: 'baseline · 19,328',
            value: 19_328,
            role: 'baseline',
          },
          {
            id: 'c1',
            label: 'candidate #1 · 7,781',
            value: 7_781,
            role: 'selected',
          },
          { id: 'run', label: 'running', value: null, role: 'running' },
        ]}
      />,
    )
    expect(html).toContain('data-sparkline')
    expect(html).toContain('aria-label="Tokens across executions"')
    expect(html).toContain('data-point-role="baseline"')
    expect(html).toContain('data-point-role="selected"')
    // The running execution is a hollow marker on the reference line, not a value.
    expect(html).toMatch(/data-point-role="running"[^>]*>/)
    expect(html).toContain('<title>running</title>')
    // One segment joins the two plotted points; the hairline spans the width.
    expect(html.match(/<line /g)).toHaveLength(2)
    expect(html).toContain('x2="100%"')
  })

  it('orients diverging bars by improvement and names the unchanged once', () => {
    const html = renderToStaticMarkup(
      <DivergingBars
        label="Relative change per test"
        groups={[
          {
            id: 'minimal_path',
            title: 'Minimal Path',
            subtitle: 'Passed → Passed · 2 of 4 metrics moved',
            unchanged: 'quality score 90 · turns 2',
            rows: [
              {
                id: 'tokens',
                label: 'tokens',
                improvement: 68.4,
                valueLabel: '-7K · -68.4%',
                tone: 'positive',
              },
              {
                id: 'duration',
                label: 'duration',
                improvement: -10.7,
                valueLabel: '+16.2s · +10.7%',
                tone: 'negative',
              },
            ],
          },
        ]}
      />,
    )
    expect(html).toContain('data-diverging-bars')
    expect(html).toContain('unchanged: quality score 90 · turns 2')
    const tokens = html.slice(html.indexOf('data-diverging-row="tokens"'))
    const duration = html.slice(html.indexOf('data-diverging-row="duration"'))
    // The improved bar starts at the reference and grows right; the regressed
    // one starts left of it. Labels sit at the tip, in text ink.
    expect(tokens).toMatch(/<rect x="58\.00%"/)
    expect(duration).toMatch(/<rect x="5[0-7]\.\d\d%"/)
    expect(tokens).toContain('text-anchor="start"')
    expect(duration).toContain('text-anchor="end"')
    expect(html).toContain('fill="var(--success, #356f3d)"')
    expect(html).toContain('fill="var(--danger, #c4001d)"')
    expect(html).toContain('>reference<')
  })

  it('collapses a dumbbell whose ends coincide into one marked point', () => {
    const html = renderToStaticMarkup(
      <Dumbbell
        label="Quality per test"
        domain={[0, 100]}
        ticks={[
          { value: 0, label: '0' },
          { value: 50, label: '50' },
          { value: 100, label: '100' },
        ]}
        rows={[
          {
            id: 'minimal_path',
            label: 'minimal path',
            baseline: 90,
            candidate: 90,
            baselineLabel: '90',
            candidateLabel: '90',
          },
          {
            id: 'persistent_state',
            label: 'persistent state',
            baseline: 85,
            candidate: 95,
            baselineLabel: '85',
            candidateLabel: '95',
          },
        ]}
      />,
    )
    expect(html).toContain('data-dumbbell')
    expect(html).toContain('90 · unchanged')
    const moved = html.slice(
      html.indexOf('data-dumbbell-row="persistent_state"'),
    )
    expect(moved).toContain('data-dumbbell-end="baseline"')
    expect(moved).toContain('data-dumbbell-end="candidate"')
    expect(moved).toContain('>85<')
    expect(moved).toContain('>95<')
  })
})
