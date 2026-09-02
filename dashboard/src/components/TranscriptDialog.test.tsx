import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  splitTranscriptTitle,
  TranscriptDialog,
} from '@/components/TranscriptDialog'

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
    // Audit TR-03 / TR-06 / TR-04: one stats row, a log that does not
    // announce itself, no "Next error" for zero errors, honest tool status.
    expect(html.match(/message<\/span>|messages<\/span>/g)).toHaveLength(1)
    expect(html).not.toContain('role="log" aria-live')
    expect(html).not.toContain('Next error')
    expect(html).not.toContain('>pending<')
    expect(html).toContain('run run-1')
    expect(html).toContain('>Reactive Automation<')
  })

  it('splits the run id out of the caller-provided title', () => {
    expect(splitTranscriptTitle('Reactive Automation · run-1')).toEqual({
      name: 'Reactive Automation',
      runId: 'run-1',
    })
    expect(splitTranscriptTitle('Plain title')).toEqual({
      name: 'Plain title',
      runId: null,
    })
  })
})
