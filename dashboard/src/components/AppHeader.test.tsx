import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { AppHeader, appHeaderActionClassName } from '@/components/AppHeader'

describe('route chrome bridge', () => {
  it('does not render a second global header', () => {
    const html = renderToStaticMarkup(
      <AppHeader active="plans" showThemeToggle={false} />,
    )

    expect(html).toBe('')
  })

  it('keeps page action classes scoped to the new shell', () => {
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

    expect(html).toBe('')
    expect(appHeaderActionClassName()).toContain('harness-e2e-header-action')
  })
})
