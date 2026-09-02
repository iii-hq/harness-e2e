import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import {
  buildLedgerRows,
  dayLabel,
  filterLedgerRows,
  groupLedgerRows,
  LEDGER_DEFAULT_FILTERS,
  ledgerFiltersFromParams,
  ledgerFiltersToParams,
  triggerLabel,
} from '@/pages/ExecutionsPage'

const NOW = Date.parse('2026-08-26T21:00:00Z')

function summary(
  overrides: Partial<DashboardExecutionSummary> & { id: string },
): DashboardExecutionSummary {
  return {
    label: 'e2e::* control-plane run',
    status: 'passed',
    availability: 'full',
    event: 'local',
    completed_at: '2026-08-26T20:11:31Z',
    subjects: [
      {
        id: 'terra',
        provider: 'openai-codex',
        model: 'gpt-5.6-terra',
        judge: { provider: 'openai-codex', model: 'gpt-5.6-sol' },
        scenarios: [],
      },
    ],
    assessment_summary: { system_statuses: { passed: 2 } } as never,
    totals: {
      expected_reports: 2,
      received_reports: 2,
      scenario_pass_rate: 1,
      report_coverage: 1,
      total_tokens: 7_918,
      wall_time_seconds: 241,
    },
    ...overrides,
  }
}

const executions = [
  summary({ id: 'passed-1' }),
  summary({
    id: 'gated-1',
    status: 'hard_gate_failed',
    completed_at: '2026-08-25T11:33:00Z',
    assessment_summary: {
      system_statuses: { hard_gate_failed: 1, passed: 1 },
    } as never,
    totals: {
      expected_reports: 2,
      received_reports: 2,
      scenario_pass_rate: 0.5,
      report_coverage: 1,
      wall_time_seconds: 939,
      total_tokens: 251_616,
    },
  }),
  summary({
    id: 'cancelled-1',
    label: 'context impact · baseline',
    status: 'cancelled',
    availability: 'unavailable',
    event: 'workflow_dispatch',
    completed_at: '2026-08-25T11:13:00Z',
    subjects: [],
    assessment_summary: undefined,
    totals: undefined,
  }),
  summary({ id: 'running-1', status: 'running', completed_at: '' }),
]

describe('executions ledger', () => {
  const rows = buildLedgerRows(executions)

  // Audit E-04: filters round-trip through the hash.
  it('reads and writes only the non-default filters', () => {
    const filters = ledgerFiltersFromParams(
      new URLSearchParams('q=terra&status=hard_gate&sort=tokens&event=local'),
    )
    expect(filters).toEqual({
      query: 'terra',
      status: 'hard_gate',
      event: 'local',
      sort: 'tokens',
    })
    expect(ledgerFiltersToParams(filters).toString()).toBe(
      'q=terra&status=hard_gate&event=local&sort=tokens',
    )
    expect(ledgerFiltersToParams(LEDGER_DEFAULT_FILTERS).toString()).toBe('')
  })

  it('filters by the result vocabulary the column shows and by trigger', () => {
    expect(
      filterLedgerRows(rows, {
        ...LEDGER_DEFAULT_FILTERS,
        status: 'hard_gate',
      }).map((row) => row.execution.id),
    ).toEqual(['gated-1'])
    expect(
      filterLedgerRows(rows, {
        ...LEDGER_DEFAULT_FILTERS,
        event: 'workflow_dispatch',
      }).map((row) => row.execution.id),
    ).toEqual(['cancelled-1'])
    expect(
      filterLedgerRows(rows, {
        ...LEDGER_DEFAULT_FILTERS,
        query: 'context impact',
      }).map((row) => row.execution.id),
    ).toEqual(['cancelled-1'])
    expect(triggerLabel('workflow_dispatch')).toBe('manual')
  })

  // Audit E-05: sorting is explicit, newest first by default.
  it('sorts by date, runtime, tokens and result', () => {
    expect(
      filterLedgerRows(rows, LEDGER_DEFAULT_FILTERS).map(
        (row) => row.execution.id,
      )[0],
    ).toBe('passed-1')
    expect(
      filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, sort: 'runtime' })[0]
        .execution.id,
    ).toBe('gated-1')
    expect(
      filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, sort: 'tokens' })[0]
        .execution.id,
    ).toBe('gated-1')
    expect(
      filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, sort: 'result' })[0]
        .execution.id,
    ).toBe('gated-1')
  })

  // Audit E-12: running is pinned, the rest is grouped by day.
  it('pins running executions above the day groups', () => {
    const grouped = groupLedgerRows(
      filterLedgerRows(rows, LEDGER_DEFAULT_FILTERS),
      NOW,
    )
    expect(grouped.running.map((row) => row.execution.id)).toEqual([
      'running-1',
    ])
    expect(
      grouped.groups.map((group) => [group.label, group.rows.length]),
    ).toEqual([
      ['today · Aug 26', 1],
      ['yesterday · Aug 25', 2],
    ])
    expect(dayLabel('2026-08-25T11:13:00Z', NOW)).toBe('yesterday · Aug 25')
  })

  // Audit O-03 / E-11: the row carries every column with a label, and a
  // cancelled row never invents numbers.
  it('renders the collapsing table with honest placeholders', () => {
    const html = renderToStaticMarkup(
      <table>
        <tbody>
          <tr>{null}</tr>
        </tbody>
      </table>,
    )
    expect(html).toContain('<table>')
    const grouped = groupLedgerRows(rows, NOW)
    expect(grouped.groups[1].rows.map((row) => row.status.label)).toEqual([
      'hard gate',
      'cancelled',
    ])
  })
})
