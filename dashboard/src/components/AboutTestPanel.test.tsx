import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  ABOUT_PANEL_STORAGE_KEY,
  AboutTestPanel,
  criteriaCaption,
  hardGateCount,
  storedAboutPanelOpen,
  TestCriteriaList,
  totalWeight,
} from '@/components/AboutTestPanel'
import type { TestSpec } from '@/lib/test-catalog'

const spec: TestSpec = {
  summary:
    'Implement fully correct standard chess rules over a frozen fixture repository.',
  prompt:
    'A pinned chess fixture repository has been copied into your workspace.\n\nImplement legal_moves(fen) and perft(fen, depth).',
  criteria: [
    {
      id: 'perft_exact',
      weight: 40,
      description:
        'Every perft position reports a node count exactly equal to the kernel oracle.',
      kind: 'required_check',
      policy: 'hard_gate',
      dimension: 'deliverable',
      source: 'deterministic',
    },
    {
      id: 'build_discipline',
      weight: 10,
      description: 'Every invocation finished within the time budget.',
      kind: 'signal',
      policy: 'advisory',
      dimension: 'efficiency',
      source: 'deterministic',
    },
  ],
  execution: {
    max_turns: 48,
    max_output_tokens: 65536,
    max_total_tokens: 1000000,
    stuck_timeout_seconds: 900,
  },
  denied_functions: ['http::*', 'browser::*'],
}

describe('about test panel', () => {
  it('states the task, the prompt, the scored contract and the limits', () => {
    const html = renderToStaticMarkup(
      <AboutTestPanel spec={spec} testId="chess_engine_build" />,
    )
    expect(html).toContain('about this test')
    expect(html).toContain('over a frozen fixture repository')
    expect(html).toContain('prompt handed to the subject')
    expect(html).toContain('perft(fen, depth)')
    expect(html).toContain('perft_exact')
    // The requirement, not an observed conclusion.
    expect(html).toContain('exactly equal to the kernel oracle')
    expect(html).toContain('hard gate')
    expect(html).toContain('score only')
    // Budgets read as numbers a person can compare, not raw tokens.
    expect(html).toContain('48 turns')
    expect(html).toContain('65,536 output')
    expect(html).toContain('900s stuck')
    expect(html).toContain('http::*')
  })

  it('offers the whole prompt without forcing the reader through it', () => {
    const html = renderToStaticMarkup(
      <AboutTestPanel spec={spec} testId="chess_engine_build" />,
    )
    // Collapsed by default: the excerpt is clipped, the full text is one click.
    // The length stands in for the fade this design system does not allow.
    expect(html).toContain('show full prompt · 3 lines')
    expect(html).toContain('aria-expanded="false"')
    expect(html).toContain('copy prompt')
  })

  it('renders backticked identifiers as code, not as literal backticks', () => {
    const html = renderToStaticMarkup(
      <AboutTestPanel
        spec={{ ...spec, summary: 'The subject is handed `engine/engine.py`.' }}
        testId="chess_engine_build"
      />,
    )
    expect(html).toContain('<code')
    expect(html).toContain('engine/engine.py')
    expect(html).not.toContain('`engine/engine.py`')
  })

  it('renders a scenario with no editorial summary from its prompt alone', () => {
    const html = renderToStaticMarkup(
      <AboutTestPanel
        spec={{ ...spec, summary: undefined }}
        testId="context_pressure"
      />,
    )
    expect(html).toContain('prompt handed to the subject')
    expect(html).toContain('perft_exact')
  })

  it('reads the collapse preference back, and treats junk as unset', () => {
    expect(storedAboutPanelOpen({ getItem: () => 'collapsed' })).toBe(false)
    expect(storedAboutPanelOpen({ getItem: () => 'open' })).toBe(true)
    expect(storedAboutPanelOpen({ getItem: () => 'yes' })).toBeNull()
    expect(storedAboutPanelOpen({ getItem: () => null })).toBeNull()
    expect(
      storedAboutPanelOpen({
        getItem: () => {
          throw new Error('storage is unavailable')
        },
      }),
    ).toBeNull()
    expect(ABOUT_PANEL_STORAGE_KEY).toBe('harness-e2e:about-test-open')
  })

  it('summarises the contract without claiming a judge that never runs', () => {
    expect(totalWeight(spec.criteria)).toBe(50)
    expect(hardGateCount(spec.criteria)).toBe(1)
    expect(criteriaCaption(spec.criteria)).toBe(
      '2 criteria · 1 hard gate · deterministic, no judge model',
    )
    expect(criteriaCaption([{ ...spec.criteria[0], source: 'judge' }])).toBe(
      '1 criterion · 1 hard gate · judge-scored',
    )
  })

  it('pairs each criterion with its outcome when a run supplies one', () => {
    const html = renderToStaticMarkup(
      <TestCriteriaList
        criteria={spec.criteria}
        outcomes={
          new Map([
            [
              'perft_exact',
              { label: 'not evaluated', status: 'unavailable' as const },
            ],
            [
              'build_discipline',
              { label: 'passed', status: 'passed' as const },
            ],
          ])
        }
      />,
    )
    expect(html).toContain('what this test required')
    expect(html).toContain('not evaluated')
    expect(html).toContain('passed')
    // Without outcomes the same list is the requirement alone.
    expect(
      renderToStaticMarkup(<TestCriteriaList criteria={spec.criteria} />),
    ).toContain('how it is scored')
  })
})
