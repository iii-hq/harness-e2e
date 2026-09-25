import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import {
  DashboardShell,
  nextHeader,
  sectionForRoute,
} from '@/components/DashboardShell'
import {
  type HeaderAction,
  HeaderActions,
  PinnedPrimary,
} from '@/components/shell/HeaderActions'

// Pane width is measured with ResizeObserver and the phone viewport with
// matchMedia at runtime; the tests choose both directly.
const layout = vi.hoisted(() => ({ narrow: false, phone: false }))
vi.mock('@/hooks/use-container-narrow', () => ({
  useContainerNarrow: () => [() => {}, layout.narrow],
}))
vi.mock('@/hooks/use-viewport-phone', () => ({
  useViewportPhone: () => layout.phone,
}))

function renderShell({ narrow = false, phone = false } = {}) {
  layout.narrow = narrow
  layout.phone = phone
  return renderToStaticMarkup(
    <DashboardShell
      route={{ page: 'workspace', view: 'executions' }}
      tabId="test"
      theme="light"
    >
      <div>content</div>
    </DashboardShell>,
  )
}

const ACTIONS: HeaderAction[] = [
  { id: 'import', label: 'Import from GitHub', onSelect: () => {} },
  { id: 'run', label: 'Run tests', primary: true, onSelect: () => {} },
]

describe('section navigation in the Console header', () => {
  it('maps every route to a section', () => {
    expect(sectionForRoute({ page: 'suites' })).toBe('suites')
    expect(sectionForRoute({ page: 'stacks' })).toBe('stacks')
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

  // Layout A: one header row, the place named once, no bar of our own.
  it('puts the title and the section tabs in the header, with no second bar', () => {
    const html = renderShell()
    const header = html.match(/<header[^>]*>.*?<\/header>/)?.[0] ?? ''
    expect(header).toContain('Harness E2E')
    expect(header).toContain('aria-label="Harness E2E sections"')
    expect(header).toContain(
      'href="#/ext/harness-e2e/executions" aria-current="page"',
    )
    for (const label of ['Tests', 'Suites', 'Stacks'])
      expect(header).toContain(`>${label}</a>`)
    expect(html).not.toContain('harness-e2e-navigation')
    expect(html).not.toContain('<select')
    expect(html).not.toContain('role="tab"')
  })

  it('keeps the skip link and the main landmark', () => {
    const html = renderShell()
    expect(html).toContain('class="skip-link" href="#harness-e2e-main"')
    expect(html).toContain('id="harness-e2e-main" tabindex="-1"')
  })

  it('switches section from a menu naming the current one below 720 px', () => {
    const html = renderShell({ narrow: true })
    expect(html).toContain('data-narrow="true"')
    expect(html).toContain('aria-label="Section: Executions"')
    expect(html).toMatch(
      /role="menuitemradio"[^>]*aria-checked="true">Executions</,
    )
    for (const label of ['Tests', 'Executions', 'Suites', 'Stacks'])
      expect(html).toContain(`>${label}<`)
    expect(html).not.toContain('harness-e2e-section-tabs')
  })

  it('opens the sections as a sheet and leaves the title out on a phone', () => {
    const html = renderShell({ phone: true })
    expect(html).not.toContain('Harness E2E<')
    expect(html).toContain('aria-label="Section: Executions"')
    expect(html).not.toContain('harness-e2e-section-tabs')
  })
})

describe('section actions in the header', () => {
  it('shows every action on a wide pane, the primary last', () => {
    const html = renderToStaticMarkup(
      <HeaderActions
        actions={ACTIONS}
        label="Execution actions"
        narrow={false}
        phone={false}
      />,
    )
    expect(html).toContain('aria-label="Execution actions"')
    expect(html).not.toContain('More actions')
    expect(html.indexOf('Import from GitHub')).toBeLessThan(
      html.indexOf('Run tests'),
    )
    expect(html).toContain('harness-e2e-header-action-primary')
  })

  it('folds secondary actions into ⋯ with full labels below 720 px', () => {
    const html = renderToStaticMarkup(
      <HeaderActions actions={ACTIONS} narrow phone={false} />,
    )
    expect(html).toContain('aria-label="More actions"')
    const menu = html.match(/<div role="menu"[^>]*>.*?<\/div><\/div>/)?.[0]
    expect(menu).toContain('Import from GitHub')
    expect(menu).not.toContain('Run tests')
    expect(html).toContain('>Run tests</button>')
  })

  it('keeps only ⋯ in the header on a phone and pins the primary below', () => {
    const header = renderToStaticMarkup(
      <HeaderActions actions={ACTIONS} narrow phone />,
    )
    expect(header).not.toContain('>Run tests</button>')
    const pinned = renderToStaticMarkup(
      <PinnedPrimary actions={ACTIONS} phone />,
    )
    expect(pinned).toContain('harness-e2e-pinned-primary')
    expect(pinned).toContain('>Run tests</button>')
    expect(
      renderToStaticMarkup(<PinnedPrimary actions={ACTIONS} phone={false} />),
    ).toBe('')
  })

  it('renders nothing when a section has no actions', () => {
    expect(
      renderToStaticMarkup(
        <HeaderActions actions={[]} narrow={false} phone={false} />,
      ),
    ).toBe('')
  })
})

describe('header updates', () => {
  const disabled: HeaderAction[] = [
    { id: 'share', label: 'Share link', disabled: true },
  ]
  const enabled: HeaderAction[] = [{ id: 'share', label: 'Share link' }]
  const header = { key: 'tests:Comparison actions:true:compare' }

  it('takes new actions under the same key', () => {
    const current = { ...header, actions: disabled }
    const next = { ...header, actions: enabled }
    expect(nextHeader(current, next)).toBe(next)
  })

  it('keeps the current header when nothing changed', () => {
    const current = { ...header, actions: enabled }
    expect(nextHeader(current, { ...header, actions: enabled })).toBe(current)
  })
})
