import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import {
  DashboardShell,
  PageActionsBar,
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
      sectionForRoute({
        page: 'execution',
        executionId: 'x',
        anchor: null,
        runId: null,
      }),
    ).toBe('executions')
    expect(sectionForRoute({ page: 'test-history', testId: 't' })).toBe('tests')
  })

  it('renders both the wide links and the narrow select', () => {
    const html = renderShell()
    expect(html).toContain('harness-e2e-navigation-wide')
    expect(html).toContain('harness-e2e-navigation-narrow')
    expect(html).toContain('aria-label="Harness E2E section"')
    expect(html).toContain('data-narrow="false"')
  })

  // Audit S-04 / A11Y-05 / A11Y-06: sections are links with aria-current,
  // the shell owns the one skip-link and the main landmark.
  it('navigates with links that mark the current section', () => {
    const html = renderShell()
    expect(html).toContain('<nav class="harness-e2e-navigation')
    expect(html).toContain('aria-label="Harness E2E sections"')
    expect(html).toContain('href="#/overview" aria-current="page"')
    expect(html).toContain('href="#/tests"')
    expect(html).not.toContain('role="tab"')
    expect(html).toContain('class="skip-link" href="#harness-e2e-main"')
    expect(html).toContain('id="harness-e2e-main" tabindex="-1"')
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

describe('page actions in the section bar', () => {
  const actions = <button type="button">new plan</button>

  // Audit S-05 / S-07: a section's primary action lives in the page, next to
  // the section links, not in the console header.
  it('renders the actions as a labelled group', () => {
    const html = renderToStaticMarkup(
      <PageActionsBar actions={actions} label="Overview actions" />,
    )
    expect(html).toContain('harness-e2e-page-actions')
    expect(html).toContain('<section class="harness-e2e-page-actions')
    expect(html).toContain('aria-label="Overview actions"')
    expect(html).toContain('>new plan<')
  })

  it('renders nothing when a page has no actions', () => {
    expect(renderToStaticMarkup(<PageActionsBar />)).toBe('')
  })
})
