import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { TranscriptDialog } from '@/components/TranscriptDialog'

describe('transcript dialog', () => {
  it('renders a console-like conversation with collapsed tool payloads', () => {
    const html = renderToStaticMarkup(
      <TranscriptDialog
        title="Reactive Automation · run-1"
        open
        onClose={() => undefined}
        messages={[
          {
            entry_id: 'user-1',
            message: {
              role: 'user',
              content: [{ type: 'text', text: 'Run the scenario.' }],
            },
          },
          {
            entry_id: 'assistant-1',
            message: {
              role: 'assistant',
              content: [
                {
                  type: 'function_call',
                  id: 'call-1',
                  function_id: 'state::get',
                  arguments: { key: 'result' },
                },
              ],
            },
          },
          {
            entry_id: 'result-1',
            message: {
              role: 'function_result',
              function_call_id: 'call-1',
              function_id: 'state::get',
              content: [{ type: 'text', text: '{"ok":true}' }],
            },
          },
        ]}
      />,
    )

    expect(html).toContain('conversation-shell')
    expect(html).toContain('session-transcript-header-content')
    expect(html).toContain('You')
    expect(html).toContain('tool')
    expect(html).toContain('Arguments')
    expect(html).toContain('Result')
    expect(html).toContain('conversation-filter')
    expect(html).not.toMatch(/<details[^>]*open/)
  })
})
