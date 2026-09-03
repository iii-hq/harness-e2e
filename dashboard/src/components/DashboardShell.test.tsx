import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import {
  DashboardShell,
  HeaderActions,
  sectionForRoute,
} from '@/components/DashboardShell'

// The shell reads the standalone theme from `document`; the embedded shell
// under test receives its theme as a prop, so the hook can be inert here.
vi.mock('@/hooks/useTheme', () => ({
  useTheme: () => ['light', () => {}],
}))

// Container width is measured with ResizeObserver at runtime; the tests
// choose the narrow state directly.
const layout = vi.hoisted(() => ({ narrow: false }))
vi.mock('@/hooks/use-container-narrow', () => ({
  useContainerNarrow: () => [() => {}, layout.narrow],
}))

function renderShell({ narrow = false } = {}) {
  layout.narrow = narrow
  return renderToStaticMarkup(
    <DashboardShell
      route={{ page: 'overview', view: 'overview' }}
      embedded={true}
      tabId="test"
      theme="light"
    >
      <div>content</div>
    </DashboardShell>,
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
    const html = renderShell()
    expect(html).toContain('harness-e2e-navigation-wide')
    expect(html).toContain('harness-e2e-navigation-narrow')
    expect(html).toContain('aria-label="Harness E2E section"')
    expect(html).toContain('data-narrow="false"')
  })

  // Audit S-01 / RD-01: visibility of the narrow select is decided by
  // dashboard-shell.css alone, keyed on data-narrow. A Tailwind `hidden`
  // utility here would win (Tailwind is imported with `important`) and the
  // navigation would disappear below 720px.
  it('lets CSS alone decide when the narrow select is visible', () => {
    const html = renderShell({ narrow: true })
    expect(html).toContain('data-narrow="true"')
    const narrowTag = html.match(
      /<div class="harness-e2e-navigation-narrow[^"]*"/,
    )?.[0]
    expect(narrowTag).toBeTruthy()
    expect(narrowTag).not.toMatch(/\bhidden\b/)
  })

  it('keeps every section reachable from the narrow select', () => {
    const html = renderShell({ narrow: true })
    for (const label of ['Overview', 'Tests', 'Executions', 'Plans']) {
      expect(html).toContain(`>${label}<`)
    }
  })

  it('names the section without a slogan in the console header', () => {
    const html = renderShell()
    expect(html).not.toContain('evidence, plans and live evaluation control')
  })
})

describe('page actions in the console header', () => {
  const actions = <button type="button">new plan</button>

  // Audit S-02 / RD-02: three inline actions pushed the console close
  // control out of a 390px viewport. In narrow containers the actions
  // collapse into one disclosure.
  it('collapses into one disclosure when the container is narrow', () => {
    const html = renderToStaticMarkup(
      <HeaderActions
        narrow={true}
        actions={actions}
        actionsLabel="Overview actions"
      />,
    )
    expect(html).toContain('<details class="harness-e2e-header-overflow">')
    expect(html).toContain('aria-label="Overview actions"')
    expect(html).toContain('harness-e2e-header-overflow-menu')
    expect(html).toContain('>new plan<')
  })

  it('stays inline when the container is wide', () => {
    const html = renderToStaticMarkup(
      <HeaderActions narrow={false} actions={actions} />,
    )
    expect(html).not.toContain('harness-e2e-header-overflow')
    expect(html).toContain('>new plan<')
  })

  it('renders nothing collapsible when a page has no actions', () => {
    const html = renderToStaticMarkup(<HeaderActions narrow={true} />)
    expect(html).not.toContain('<details')
  })
})
