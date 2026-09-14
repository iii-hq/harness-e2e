import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'

describe('route chrome bridge', () => {
  it('does not render a second global header', () => {
    const html = renderToStaticMarkup(<DashboardPageActions active="plans" />)

    expect(html).toBe('')
  })

  it('keeps page action classes scoped to the new shell', () => {
    const html = renderToStaticMarkup(
      <DashboardPageActions
        active="executions"
        actionsLabel="Execution actions"
        actions={
          <button type="button" className={dashboardHeaderActionClassName()}>
            Quick execution
          </button>
        }
      />,
    )

    expect(html).toBe('')
    expect(dashboardHeaderActionClassName()).toContain(
      'harness-e2e-header-action',
    )
  })
})
