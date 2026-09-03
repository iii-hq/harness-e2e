import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  matchesQuery,
  relativeTime,
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

    // Audit TR-02: a role rail on a left-aligned column, no legacy classes.
    expect(html).toContain('data-transcript')
    expect(html).toContain('data-role="user"')
    expect(html).toContain('data-kind="tool"')
    expect(html).toContain('>user<')
    expect(html).toContain('>tool<')
    expect(html).not.toContain('conversation-shell')
    expect(html).not.toContain('conversation-filter')
    // Audit TR-03: search, copy on every payload, no zero-error control.
    expect(html).toContain('Search the transcript')
    expect(html).toContain('Copy this message')
    expect(html).toContain('Copy the arguments')
    expect(html).toContain('Copy the result')
    expect(html).toContain('2 of 2 events shown')
    expect(html).not.toContain('next error')
    expect(html).not.toMatch(/<details[^>]*open/)
    expect(html).toContain('>arguments<')
    expect(html).toContain('>result<')
    expect(html).not.toContain('>pending<')
    expect(html).toContain('run run-1')
    expect(html).toContain('>Reactive Automation<')
  })

  it('times events against the first one and filters by text', () => {
    const origin = Date.parse('2026-08-26T20:07:31Z')
    expect(relativeTime('2026-08-26T20:07:36Z', origin)).toBe('+5s')
    expect(relativeTime('2026-08-26T20:09:03Z', origin)).toBe('+1m 32s')
    expect(relativeTime(null, origin)).toBeNull()
    expect(
      matchesQuery({ id: 'a', kind: 'message', text: 'Run it' }, 'run'),
    ).toBe(true)
    expect(
      matchesQuery({ id: 'a', kind: 'message', text: 'Run it' }, 'missing'),
    ).toBe(false)
    expect(
      matchesQuery(
        { id: 'b', kind: 'tool', functionId: 'state::get' },
        'state',
      ),
    ).toBe(true)
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
