import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { ScenarioMatrixItem } from '@/lib/scenario-matrix'
import type { TranscriptEvent } from '@/lib/transcript-view'
import { attentionItems, NeedsAttention } from './NeedsAttention'
import { ScreenshotViewer } from './screenshots'
import {
  errorsByCause,
  TranscriptPage,
  transcriptBlocks,
} from './TranscriptPage'

const tool = (
  id: string,
  functionId: string,
  isError = false,
  text = '',
): TranscriptEvent => ({
  id,
  kind: 'tool',
  functionId,
  isError,
  status: isError ? 'error' : 'completed',
  result: { text, details: null },
})
const message = (id: string, role: string, text: string): TranscriptEvent => ({
  id,
  kind: 'message',
  role,
  text,
})

describe('transcript page', () => {
  const events = [
    message('m1', 'user', 'Build the worker'),
    tool('t1', 'coder::read-file'),
    tool('t2', 'shell::exec', true, 'exit 1\nstack'),
    tool('t3', 'shell::exec', true, 'exit 1'),
    message('m2', 'assistant', 'Done'),
    tool('t4', 'browser::act', true, 'timeout'),
  ]

  it('groups consecutive tool calls between messages', () => {
    const blocks = transcriptBlocks(events)
    expect(blocks.map((block) => block.kind)).toEqual([
      'message',
      'tools',
      'message',
      'tools',
    ])
    expect(blocks[1].kind === 'tools' && blocks[1].events.length).toBe(3)
  })

  it('groups errors by function and first line, most frequent first', () => {
    expect(errorsByCause(events)).toEqual([
      { cause: 'exit 1', functionId: 'shell::exec', ids: ['t2', 't3'] },
      { cause: 'timeout', functionId: 'browser::act', ids: ['t4'] },
    ])
  })

  it('renders the run line, outline, errors by cause and actions', () => {
    const html = renderToStaticMarkup(
      <TranscriptPage
        title="form_flow_build"
        runLine="run 7e450dca · claude-code/opus · 8m 00s"
        messages={[
          { message: { role: 'user', content: 'Build the worker' } },
          {
            message: {
              role: 'assistant',
              content: [
                {
                  type: 'function_call',
                  id: 'c1',
                  function_id: 'shell::exec',
                  arguments: {},
                },
              ],
            },
          },
          {
            message: {
              role: 'function_result',
              function_call_id: 'c1',
              is_error: true,
              content: 'exit 1',
            },
          },
        ]}
        backHref="#/back"
        backLabel="Back to the evidence record"
      />,
    )
    expect(html).toContain('1 messages · 1 tool calls · 1 errors')
    expect(html).toContain('Outline')
    expect(html).toContain('Errors by cause')
    expect(html).toContain('Next error (0/1)')
    expect(html).toContain('Copy all')
    expect(html).toContain('Download JSON')
    expect(html).toContain('Consecutive tool calls are grouped')
  })
})

describe('needs attention', () => {
  const item = (key: string, status: string, reason: string | null) =>
    ({
      key,
      scenarioId: key,
      objective: { status, label: status, raw: status },
      reason,
    }) as unknown as ScenarioMatrixItem

  it('lists what did not pass with why, then the warnings', () => {
    const items = attentionItems(
      [
        item('minimal_path', 'passed', null),
        item('timer_wake', 'failed', 'Criterion “wakes” missed'),
      ],
      ['No provider_env_file is configured'],
    )
    expect(items.map((entry) => entry.kind)).toEqual(['test', 'warning'])
    const html = renderToStaticMarkup(
      <NeedsAttention items={items} onShow={() => {}} />,
    )
    expect(html).toContain('Needs attention')
    expect(html).toContain('timer_wake')
    expect(html).toContain('Criterion “wakes” missed')
    expect(html).toContain('Show test')
    expect(html).not.toContain('minimal_path')
  })

  it('renders nothing when everything passed', () => {
    expect(
      renderToStaticMarkup(<NeedsAttention items={[]} onShow={() => {}} />),
    ).toBe('')
  })
})

describe('screenshot viewer', () => {
  it('opens full size with fit, zoom, previous/next and Esc', () => {
    const shots = [
      {
        key: 'a',
        executionId: 'e',
        runId: 'r',
        path: 'p',
        pointer: '/0',
        caption: 'form filled',
      },
      {
        key: 'b',
        executionId: 'e',
        runId: 'r',
        path: 'p',
        pointer: '/1',
        caption: 'saved',
      },
    ]
    const html = renderToStaticMarkup(
      <ScreenshotViewer
        screenshots={shots}
        images={{ a: { source: 'data:image/png;base64,AA==' } }}
        index={0}
        onIndex={() => {}}
        onClose={() => {}}
      />,
    )
    expect(html).toContain('role="dialog"')
    expect(html).toContain('1 of 2')
    expect(html).toContain('>Fit<')
    expect(html).toContain('aria-label="Zoom in"')
    expect(html).toContain('aria-label="Next screenshot"')
    expect(html).toContain('<kbd>Esc</kbd> close')
    expect(html).toContain('aria-label="Screenshots of this run"')
    expect(html).toContain('aria-label="Download form filled"')
  })
})
