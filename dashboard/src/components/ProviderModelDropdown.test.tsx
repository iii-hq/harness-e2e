import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ProviderModelMenu } from '@/components/ProviderModelDropdown'

const groups = [
  {
    provider: 'anthropic',
    models: [
      { label: 'claude-sonnet-5', value: 'anthropic\nclaude-sonnet-5' },
      { label: 'claude-opus-5', value: 'anthropic\nclaude-opus-5' },
    ],
  },
  {
    provider: 'openai',
    models: [{ label: 'gpt-5', value: 'openai\ngpt-5' }],
  },
]

describe('provider model menu', () => {
  // Audit PN-07 / RS-08: providers start expanded, groups are labelled and
  // the provider toggles stay out of the tab order.
  it('lists every model with providers expanded by default', () => {
    const html = renderToStaticMarkup(
      <ProviderModelMenu
        id="menu"
        ariaLabel="Execution model"
        groups={groups}
        value={'openai\ngpt-5'}
        collapsedProviders={new Set()}
        onToggleProvider={() => undefined}
        onSelect={() => undefined}
      />,
    )
    expect(html.match(/role="option"/g)).toHaveLength(3)
    expect(html.match(/role="group"/g)).toHaveLength(2)
    expect(html).toContain('aria-expanded="true"')
    expect(html).toContain('tabindex="-1"')
    expect(html).toContain('aria-selected="true"')
  })

  // Audit PN-08: a chosen judge can go back to the default.
  it('offers a clear option first when asked', () => {
    const html = renderToStaticMarkup(
      <ProviderModelMenu
        id="menu"
        ariaLabel="Judge model"
        groups={groups}
        value=""
        clearLabel="Default judge (automatic)"
        collapsedProviders={new Set(['anthropic'])}
        onToggleProvider={() => undefined}
        onSelect={() => undefined}
      />,
    )
    expect(html.indexOf('Default judge (automatic)')).toBeLessThan(
      html.indexOf('claude') === -1 ? html.length : html.indexOf('claude'),
    )
    expect(html).toContain('aria-selected="true"')
    expect(html.match(/role="option"/g)).toHaveLength(2)
    expect(html).toContain('aria-expanded="false"')
  })
})
