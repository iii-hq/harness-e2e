import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { VerdictCard } from '@/components/compare/VerdictCard'
import {
  changeKey,
  type ExecutionVerdict,
  repetitionsText,
  verdictHeadline,
  verdictTone,
} from '@/lib/verdict'
import { cannotTell } from '@/test-fixtures/release-overview'

const pair: ExecutionVerdict = {
  ...cannotTell,
  warnings: [
    'B: 1 counted run has no subject cost and leaves the cost: registry_implementation ×1.',
  ],
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
  criteria: {
    min_runs: 9,
    configurations: [
      {
        side: 'b',
        scenario: 'kanban_c3_board',
        behavior: 'sha256:c3',
        model: 'deepseek/deepseek-flash',
        stack: { harness: '1.8.50', iii: '0.24.6' },
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
            not_evaluated: 0,
            rate: 0.6,
            low: 0.23,
            high: 0.88,
            class: 'insufficient_data',
          },
        ],
        not_evaluated: ['criterion_precision'],
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
        not_evaluated: { a: 0, b: 0 },
        runs: { a: 5, b: 5 },
        points: { a: 35, b: 21 },
        delta: -14,
      },
      {
        scenario: 'registry_verification',
        behavior: 'sha256:rv',
        criteria: ['verification.precision'],
        failing: { a: 0, b: 0 },
        not_evaluated: { a: 0, b: 2 },
        runs: { a: 4, b: 5 },
        points: { a: 20, b: 12 },
        delta: -8,
      },
    ],
    warnings: [
      'A: 30 of 30 runs have no stack in their results: criterion health cannot tell their configuration apart, nor the comparison whether the side mixes them.',
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
    expect(verdictHeadline({ ...cannotTell, qualifier: 'test_inferior' })).toBe(
      'Cannot tell yet: a test fell beyond the margin on its own',
    )
    expect(verdictTone({ ...cannotTell, qualifier: 'test_inferior' })).toBe(
      'negative',
    )
    const tradeOff = { ...cannotTell, qualifier: 'trade_off' as const }
    expect(verdictHeadline(tradeOff)).toBe(
      'Cannot tell yet: quality improved and the cost rose, a trade-off the rule does not weigh',
    )
    expect(verdictHeadline({ ...cannotTell, verdict: null })).toBe(
      'No calibrated verdict',
    )
    expect(verdictTone({ ...cannotTell, verdict: 'worse' })).toBe('negative')
    expect(verdictTone({ ...cannotTell, qualifier: 'quality_fell' })).toBe(
      'negative',
    )
    // A trade-off is no fall.
    expect(verdictTone(tradeOff)).toBe('neutral')
    expect(verdictTone(cannotTell)).toBe('neutral')
  })

  it('says how many repetitions are missing, with a ceiling, or why there is no number', () => {
    expect(repetitionsText(cannotTell)).toBe(
      'At least 4 runs per test on A, 5 on B · σ 15.3 points per run; more than 30 runs per test on each side would be needed to settle the 5-point margin.',
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
      'about 12 runs per test on each side, those made kept, would settle the 5-point margin with a predicted 80%: 8 more.',
    )
    const both = {
      ...some,
      repetitions: {
        ...some.repetitions,
        settles: ['quality' as const, 'cost' as const],
      },
    }
    expect(repetitionsText(both)).toContain(
      'would settle the 5-point margin and the cost with a predicted 80%',
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
      'Cost -19.5%: cheaper (p &lt; 0.001).',
      'more than 30 runs per test',
      'B: 1 counted run has no subject cost and leaves the cost',
      'criterion_empty, criterion_retry, criterion_total, criterion_settings: short in 0 of 5 runs on A, 2 of 5 on B, −14.0 points',
      'verification.precision: short in 0 of 4 runs on A, 0 of 5 on B, not evaluated in 0 on A and 2 on B, −8.0 points',
      'A: 30 of 30 runs have no stack in their results',
      'Criterion health: 1 configuration · 1 with fewer than 9 counted runs: insufficient data · 1 criterion no counted run evaluated.',
      'Over every counted run of both executions',
      'Quality within 5 points, cost within ±5%: permutation tests within each test, one-sided α 0.025 (improved, fell, cheaper and more expensive at α / 2; each test&#x27;s fall beyond the margin Holm-adjusted), 10,000 permutations, seed 0.',
    ])
      expect(html).toContain(text)
    expect(html).not.toContain('technically valid')
    expect(
      renderToStaticMarkup(
        <VerdictCard load={{ value: null, error: 'execution not found' }} />,
      ),
    ).toContain('The verdict could not be read: execution not found')
    expect(
      renderToStaticMarkup(<VerdictCard load={{ value: null, error: null }} />),
    ).toContain('Reading the verdict…')
    const waiting = renderToStaticMarkup(
      <VerdictCard load={{ value: null, error: null, waiting: true }} />,
    )
    expect(waiting).toContain('data-comparison-verdict="waiting"')
    expect(waiting).toContain(
      'The verdict is read once both executions finish.',
    )
  })

  it('keys a change by its definition too: a scenario can move under two', () => {
    const change = pair.criteria.changes[0]
    expect(changeKey(change)).not.toBe(
      changeKey({ ...change, behavior: 'sha256:c3-new' }),
    )
  })
})
