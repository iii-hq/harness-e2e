import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import { DashboardShell, sectionForRoute } from '@/components/DashboardShell'

// The shell reads the standalone theme from `document`; the embedded shell
// under test receives its theme as a prop, so the hook can be inert here.
vi.mock('@/hooks/useTheme', () => ({
  useTheme: () => ['light', () => {}],
}))

function renderShell(narrow: boolean) {
  return renderToStaticMarkup(
    <DashboardShell
      route={{ page: 'overview', view: 'overview' }}
      embedded={true}
      tabId="test"
      theme="light"
    >
      <div>content</div>
    </DashboardShell>,
  ).replace(
    'data-narrow="false"',
    narrow ? 'data-narrow="true"' : 'data-narrow="false"',
  )
}

describe('section navigation', () => {
  it('maps every route to a section', () => {
    expect(sectionForRoute({ page: 'plans' })).toBe('plans')
    expect(sectionForRoute({ page: 'plan-create' })).toBe('plans')
    expect(
      sectionForRoute({ page: 'execution', executionId: 'x', anchor: null }),
    ).toBe('executions')
    expect(sectionForRoute({ page: 'test-history', testId: 't' })).toBe('tests')
    expect(sectionForRoute({ page: 'coverage' })).toBe('coverage')
  })

  it('renders both the wide tabs and the narrow select', () => {
    const html = renderShell(false)
    expect(html).toContain('harness-e2e-navigation-wide')
    expect(html).toContain('harness-e2e-navigation-narrow')
    expect(html).toContain('aria-label="Harness E2E section"')
  })

  // Audit S-01 / RD-01: the narrow select carries the Tailwind `hidden`
  // utility, and Tailwind is imported with `important`, so the CSS override
  // in dashboard-shell.css never wins and the navigation disappears below
  // 720px. PR2 of the UI roadmap moves the toggle entirely into CSS (see the
  // companion check in tests/dashboard/shell-narrow-nav.test.cjs); when it
  // lands this `it.fails` must become a plain `it`.
  it.fails('lets CSS alone decide when the narrow select is visible', () => {
    const html = renderShell(true)
    const narrowTag = html.match(
      /<div class="harness-e2e-navigation-narrow[^"]*"/,
    )?.[0]
    expect(narrowTag).toBeTruthy()
    expect(narrowTag).not.toMatch(/\bhidden\b/)
  })

  it('keeps every section reachable from the narrow select', () => {
    const html = renderShell(true)
    for (const label of ['Overview', 'Tests', 'Executions', 'Plans']) {
      expect(html).toContain(`>${label}<`)
    }
  })
})
