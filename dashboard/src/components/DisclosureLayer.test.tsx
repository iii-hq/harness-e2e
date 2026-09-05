import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { DisclosureLayer } from '@/components/DisclosureLayer'

describe('disclosure layer', () => {
  it('carries its scent while closed so hiding never means losing', () => {
    const html = renderToStaticMarkup(
      <DisclosureLayer
        id="results"
        label="scenario results"
        scent="minimal path v2 passed · 2m 47s · persistent state v1 passed · 2m 08s"
        open={false}
      >
        <p>body</p>
      </DisclosureLayer>,
    )
    expect(html).toContain('id="results"')
    expect(html).toContain('scenario results')
    expect(html).toContain('data-layer-scent')
    expect(html).toContain('persistent state v1 passed')
    expect(html).not.toContain('open=""')
  })

  it('opens from its prop, keeps the label and swaps the scent for actions', () => {
    const html = renderToStaticMarkup(
      <DisclosureLayer
        id="summary"
        label="what happened and next step"
        scent="passed on infrastructure and execution"
        open
        actions={<a href="#evidence">inspect retained evidence</a>}
      >
        <p>body</p>
      </DisclosureLayer>,
    )
    expect(html).toContain('open=""')
    expect(html).toContain('what happened and next step')
    expect(html).toContain('inspect retained evidence')
    // The scent is still in the markup, hidden by the open state, so a reader
    // who closes the layer again gets it back without a re-render.
    expect(html).toContain('group-open:hidden')
  })
})
