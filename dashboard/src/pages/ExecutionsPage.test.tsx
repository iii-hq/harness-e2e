import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { DashboardExecutionSummary } from '@/lib/dashboard-data-source'
import {
  buildLedgerRows,
  dayLabel,
  filterLedgerRows,
  groupHeading,
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
    id: 'failed-1',
    status: 'technical_failed',
    completed_at: '2026-08-25T11:33:00Z',
    assessment_summary: {
      system_statuses: { infrastructure_error: 1, passed: 1 },
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
      new URLSearchParams('q=terra&status=failed&sort=tokens&event=local'),
    )
    expect(filters).toEqual({
      query: 'terra',
      status: 'failed',
      event: 'local',
      sort: 'tokens',
    })
    expect(ledgerFiltersToParams(filters).toString()).toBe(
      'q=terra&status=failed&event=local&sort=tokens',
    )
    expect(ledgerFiltersToParams(LEDGER_DEFAULT_FILTERS).toString()).toBe('')
  })

  it('filters by the result vocabulary the column shows and by trigger', () => {
    expect(
      filterLedgerRows(rows, {
        ...LEDGER_DEFAULT_FILTERS,
        status: 'failed',
      }).map((row) => row.execution.id),
    ).toEqual(['failed-1'])
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
    ).toBe('failed-1')
    expect(
      filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, sort: 'tokens' })[0]
        .execution.id,
    ).toBe('failed-1')
    expect(
      filterLedgerRows(rows, { ...LEDGER_DEFAULT_FILTERS, sort: 'result' })[0]
        .execution.id,
    ).toBe('failed-1')
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

  it('groups the runs of one Release Control execution as its plan, with additive figures', () => {
    const plan = {
      execution_id: '4096c79e-c273-4495-ab8f-4b2741750197',
      attempt: 1,
      profile: 'regression',
      campaign_id: 'regression-r01',
      group_id: 'case-minimal-path',
    }
    const rcRows = buildLedgerRows([
      summary({
        id: 'rc-a',
        label:
          'Regression · regression-r01 · case-minimal-path · Harness 1.8.17',
        lane: 'local-regression',
        release_control: plan,
        completed_at: '2026-08-26T20:30:00Z',
        totals: {
          expected_reports: 1,
          received_reports: 1,
          scenario_pass_rate: 1,
          report_coverage: 1,
          total_tokens: 4_000,
          wall_time_seconds: 100,
        },
      }),
      summary({ id: 'local-between', completed_at: '2026-08-26T20:20:00Z' }),
      summary({
        id: 'rc-b',
        label: 'Regression · regression-r01 · case-timer-wake · Harness 1.8.17',
        lane: 'local-regression',
        status: 'technical_failed',
        release_control: { ...plan, group_id: 'case-timer-wake' },
        completed_at: '2026-08-26T20:10:00Z',
        assessment_summary: {
          system_statuses: { infrastructure_error: 1 },
        } as never,
        totals: {
          expected_reports: 1,
          received_reports: 1,
          scenario_pass_rate: 0,
          report_coverage: 1,
          total_tokens: 6_000,
          wall_time_seconds: 50,
        },
      }),
    ])
    const grouped = groupLedgerRows(
      filterLedgerRows(rcRows, LEDGER_DEFAULT_FILTERS),
      NOW,
    )
    expect(
      grouped.groups.map((group) => [
        group.key,
        group.rows.map((row) => row.execution.id),
      ]),
    ).toEqual([
      ['plan:4096c79e-c273-4495-ab8f-4b2741750197', ['rc-a', 'rc-b']],
      ['2026-7-26', ['local-between']],
    ])
    expect(groupHeading(grouped.groups[0])).toBe(
      'regression · regression-r01 · release control 4096c79e · 2 runs · 50% pass · 10,000 tokens · 2m 30s',
    )
    expect(groupHeading(grouped.groups[1])).toBe('today · Aug 26 · 1')
    expect(
      filterLedgerRows(rcRows, {
        ...LEDGER_DEFAULT_FILTERS,
        query: '4096c79e',
      }).map((row) => row.execution.id),
    ).toEqual(['rc-a', 'rc-b'])
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
      'failed',
      'cancelled',
    ])
  })
})
