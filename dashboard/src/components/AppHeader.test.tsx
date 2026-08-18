import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { AppHeader, appHeaderActionClassName } from '@/components/AppHeader'

describe('application header', () => {
  it('keeps global destinations stable and identifies the active page', () => {
    const html = renderToStaticMarkup(
      <AppHeader active="plans" showThemeToggle={false} />,
    )

    for (const label of [
      'Overview',
      'Tests',
      'Executions',
      'Plans',
      'Coverage',
    ]) {
      expect(html).toContain(`>${label}</a>`)
    }
    expect(html).toContain('href="#/plans" aria-current="page"')
    expect(html).toContain('aria-label="Dashboard"')
  })

  it('provides an accessible slot for page-specific actions', () => {
    const html = renderToStaticMarkup(
      <AppHeader
        active="overview"
        actionsLabel="Overview actions"
        showThemeToggle={false}
        actions={
          <button type="button" className={appHeaderActionClassName()}>
            Quick execution
          </button>
        }
      />,
    )

    expect(html).toContain('aria-label="Overview actions"')
    expect(html).toContain('Quick execution')
    expect(html).toContain('motion-reduce:transition-none')
  })
})
