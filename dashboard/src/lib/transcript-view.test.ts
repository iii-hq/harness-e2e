import { describe, expect, it } from 'vitest'
import {
  formatTranscriptPayload,
  normalizeTranscript,
  transcriptSummary,
} from '@/lib/transcript-view'

describe('transcript presentation', () => {
  it('joins messages and attaches function results to their calls', () => {
    const events = normalizeTranscript([
      {
        entry_id: 'user-1',
        message: {
          role: 'user',
          content: [{ type: 'text', text: 'Run the case.' }],
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
          details: { ok: true },
        },
      },
    ])
    expect(events).toHaveLength(2)
    expect(events[1]).toMatchObject({
      kind: 'tool',
      functionId: 'state::get',
      status: 'completed',
      isError: false,
    })
    expect(transcriptSummary(events)).toEqual({
      messages: 1,
      calls: 1,
      errors: 0,
    })
  })

  it('keeps tool payloads readable and unwraps delegated calls', () => {
    const events = normalizeTranscript([
      {
        entry_id: 'assistant-1',
        message: {
          role: 'assistant',
          content: [
            {
              type: 'function_call',
              id: 'call-1',
              function_id: 'agent_trigger',
              arguments: {
                function: 'database.query',
                payload: { sql: 'select 1' },
              },
            },
          ],
        },
      },
    ])
    expect(events[0]).toMatchObject({
      functionId: 'database.query',
      arguments: { sql: 'select 1' },
    })
    expect(formatTranscriptPayload('{"ok":true}')).toBe(
      `{
  "ok": true
}`,
    )
  })
})
