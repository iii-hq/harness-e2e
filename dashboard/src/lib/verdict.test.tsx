import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { VerdictCard } from '@/components/compare/VerdictCard'
import {
  type ExecutionVerdict,
  repetitionsText,
  verdictHeadline,
  verdictTone,
} from '@/lib/verdict'
import { cannotTell } from '@/test-fixtures/release-overview'

const pair: ExecutionVerdict = {
  ...cannotTell,
  figures: [],
  tests: [],
  dropped: [],
  runs: {
    a: {
      counted: 28,
      excluded: 2,
      causes: { evaluated: 28, e2e_infrastructure: 2 },
    },
    b: {
      counted: 29,
      excluded: 1,
      causes: { evaluated: 29, evaluation_environment: 1 },
    },
  },
  warnings: [
    'kanban_c3_board fell 16.0 points, more than the 5-point margin (its own p = 0.220).',
  ],
  criteria: {
    min_runs: 10,
    configurations: [
      {
        side: 'b',
        scenario: 'kanban_c3_board',
        behavior: 'sha256:c3',
        runs: {
          evaluated: 5,
          zeroed: 0,
          prerequisite_failed: 0,
          infrastructure: 0,
        },
        sufficient: false,
        criteria: [
          {
            id: 'criterion_empty',
            full: 3,
            n: 5,
            rate: 0.6,
            low: 0.23,
            high: 0.88,
            class: 'insufficient_data',
          },
        ],
        groups: [{ criteria: ['criterion_empty'], failing: 2, of: 5 }],
      },
    ],
    changes: [
      {
        scenario: 'kanban_c3_board',
        behavior: 'sha256:c3',
        criteria: [
          'criterion_empty',
          'criterion_retry',
          'criterion_total',
          'criterion_settings',
        ],
        failing: { a: 0, b: 2 },
        runs: { a: 5, b: 5 },
        points: { a: 35, b: 21 },
        delta: -14,
      },
    ],
  },
}

describe('calibrated verdict', () => {
  it('says the four outputs, the qualifier and that there is none', () => {
    expect(verdictHeadline(cannotTell)).toBe('Cannot tell yet')
    expect(verdictHeadline({ ...cannotTell, verdict: 'better' })).toBe('Better')
    expect(verdictHeadline({ ...cannotTell, verdict: 'no_difference' })).toBe(
      'No difference',
    )
    expect(verdictHeadline({ ...cannotTell, qualifier: 'quality_fell' })).toBe(
      'Cannot tell yet: quality fell, by an amount the runs cannot bound',
    )
    expect(verdictHeadline({ ...cannotTell, verdict: null })).toBe(
      'No calibrated verdict',
    )
    expect(verdictTone({ ...cannotTell, verdict: 'worse' })).toBe('negative')
    expect(verdictTone(cannotTell)).toBe('neutral')
  })

  it('says how many repetitions are missing, with a ceiling, or why there is no number', () => {
    expect(repetitionsText(cannotTell)).toBe(
      'At least 4 runs per test on A, 4 on B · σ 9.1 points per run; more than 30 runs per test on each side would be needed to settle the 5-point margin.',
    )
    const some = {
      ...cannotTell,
      repetitions: {
        ...cannotTell.repetitions,
        more_than: null,
        needed: 12,
        missing: 8,
      },
    }
    expect(repetitionsText(some)).toContain(
      'about 12 runs per test on each side would settle the 5-point margin: 8 more.',
    )
    const none = {
      ...cannotTell,
      repetitions: {
        ...cannotTell.repetitions,
        more_than: null,
        note: 'No number: the observed drop already reaches the 5-point margin.',
      },
    }
    expect(repetitionsText(none)).toMatch(/points per run\. No number/)
  })

  it('shows the verdict of a pair with its reasons, warnings and the criteria that moved the score', () => {
    const html = renderToStaticMarkup(
      <VerdictCard load={{ value: pair, error: null }} />,
    )
    for (const text of [
      'data-comparison-verdict="cannot_tell"',
      'Cannot tell yet',
      'Cost -19.8%: cheaper (p &lt; 0.001).',
      'more than 30 runs per test',
      'kanban_c3_board fell 16.0 points',
      'criterion_empty, criterion_retry, criterion_total, criterion_settings: short in 0 of 5 runs on A, 2 of 5 on B',
      '−14.0 points',
      'Criterion health: 1 configuration · 1 with fewer than 10 counted runs: insufficient data.',
      'Quality within 5 points, cost within ±5%: permutation tests within each test, one-sided α 0.025, 10,000 permutations, seed 0.',
    ])
      expect(html).toContain(text)
    expect(
      renderToStaticMarkup(
        <VerdictCard load={{ value: null, error: 'execution not found' }} />,
      ),
    ).toContain('The verdict could not be read: execution not found')
    expect(
      renderToStaticMarkup(<VerdictCard load={{ value: null, error: null }} />),
    ).toContain('Reading the verdict…')
  })
})
