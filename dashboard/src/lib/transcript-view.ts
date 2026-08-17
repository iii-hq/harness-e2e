export type TranscriptEvent = {
  id: string
  kind: 'message' | 'tool'
  role?: string
  text?: string
  callId?: string | null
  functionId?: string
  arguments?: unknown
  result?: { text: string; details: unknown }
  isError?: boolean
  status?: string
  timestamp?: string | number | null
  model?: string | null
  provider?: string | null
}

function contentBlocks(content: unknown): string[] {
  if (typeof content === 'string') return content.trim() ? [content.trim()] : []
  if (!Array.isArray(content)) return []
  return content
    .filter(
      (block) =>
        block &&
        typeof block === 'object' &&
        (block as Record<string, unknown>).type === 'text',
    )
    .map((block) => String((block as Record<string, unknown>).text ?? ''))
    .filter(Boolean)
}

function textFrom(content: unknown): string {
  return contentBlocks(content).join('\n\n')
}

function displayFunctionId(functionId: unknown, argumentsValue: unknown) {
  const rawId = typeof functionId === 'string' ? functionId.trim() : ''
  if (
    rawId === 'agent_trigger' &&
    argumentsValue &&
    typeof argumentsValue === 'object' &&
    typeof (argumentsValue as Record<string, unknown>).function === 'string'
  ) {
    return String((argumentsValue as Record<string, unknown>).function).trim()
  }
  return rawId || 'unknown function'
}

function displayFunctionArguments(
  functionId: unknown,
  argumentsValue: unknown,
) {
  if (
    functionId === 'agent_trigger' &&
    argumentsValue &&
    typeof argumentsValue === 'object'
  ) {
    return (argumentsValue as Record<string, unknown>).payload ?? null
  }
  return argumentsValue ?? null
}

function parseJson(value: string): unknown {
  try {
    return JSON.parse(value)
  } catch {
    return value
  }
}

export function formatTranscriptPayload(value: unknown): string {
  if (value == null) return ''
  const parsed = typeof value === 'string' ? parseJson(value.trim()) : value
  if (typeof parsed === 'string') return parsed
  try {
    return JSON.stringify(parsed, null, 2)
  } catch {
    return String(parsed)
  }
}

function entryId(entry: Record<string, unknown>, fallback: string) {
  return String(entry.entry_id ?? fallback).replace(/[^a-zA-Z0-9_-]/g, '-')
}

export function normalizeTranscript(messages: unknown): TranscriptEvent[] {
  const events: TranscriptEvent[] = []
  const calls = new Map<string, TranscriptEvent>()
  if (!Array.isArray(messages)) return events
  messages.forEach((entryValue, index) => {
    if (!entryValue || typeof entryValue !== 'object') return
    const entry = entryValue as Record<string, unknown>
    const message =
      entry.message && typeof entry.message === 'object'
        ? (entry.message as Record<string, unknown>)
        : null
    const role = String(message?.role ?? '')
    const baseId = entryId(entry, `entry-${index + 1}`)
    if (role === 'user' || role === 'assistant') {
      const text = textFrom(message?.content)
      if (text)
        events.push({
          id: `${baseId}-message`,
          kind: 'message',
          role,
          text,
          timestamp: (message?.timestamp as string | number | null) ?? null,
          model: (message?.model as string | null) ?? null,
          provider: (message?.provider as string | null) ?? null,
        })
      if (role === 'assistant' && Array.isArray(message?.content)) {
        message.content.forEach((blockValue, blockIndex) => {
          if (!blockValue || typeof blockValue !== 'object') return
          const block = blockValue as Record<string, unknown>
          if (block.type !== 'function_call') return
          const callId = String(block.id ?? `${baseId}-call-${blockIndex + 1}`)
          const event: TranscriptEvent = {
            id: `${baseId}-${callId}`.replace(/[^a-zA-Z0-9_-]/g, '-'),
            kind: 'tool',
            callId,
            functionId: displayFunctionId(block.function_id, block.arguments),
            arguments: displayFunctionArguments(
              block.function_id,
              block.arguments,
            ),
            timestamp: (message.timestamp as string | number | null) ?? null,
            result: undefined,
            isError: false,
            status: 'pending',
          }
          events.push(event)
          calls.set(callId, event)
        })
      }
      return
    }
    if (role === 'function_result') {
      const callId = String(message?.function_call_id ?? '')
      const result = {
        text: textFrom(message?.content),
        details: message?.details ?? null,
      }
      const event = callId ? calls.get(callId) : undefined
      if (event) {
        event.result = result
        event.isError = message?.is_error === true
        event.status = event.isError ? 'error' : 'completed'
      } else {
        events.push({
          id: `${baseId}-result`,
          kind: 'tool',
          callId: callId || null,
          functionId: displayFunctionId(message?.function_id, null),
          arguments: null,
          result,
          isError: message?.is_error === true,
          status: message?.is_error === true ? 'error' : 'completed',
          timestamp: (message?.timestamp as string | number | null) ?? null,
        })
      }
      return
    }
    if (entry.custom && typeof entry.custom === 'object') {
      const custom = entry.custom as Record<string, unknown>
      if (custom.type !== 'function_call') return
      const status = String(custom.status ?? 'completed').toLowerCase()
      events.push({
        id: `${baseId}-custom`,
        kind: 'tool',
        callId: null,
        functionId: displayFunctionId(custom.name, custom.arguments),
        arguments: displayFunctionArguments(custom.name, custom.arguments),
        timestamp: (custom.timestamp as string | number | null) ?? null,
        result:
          custom.result != null
            ? { text: '', details: custom.result }
            : undefined,
        isError: status === 'failed' || status === 'error',
        status,
      })
    }
  })
  return events
}

export function transcriptSummary(events: TranscriptEvent[]) {
  return events.reduce(
    (summary, event) => ({
      messages: summary.messages + (event.kind === 'message' ? 1 : 0),
      calls: summary.calls + (event.kind === 'tool' ? 1 : 0),
      errors: summary.errors + (event.kind === 'tool' && event.isError ? 1 : 0),
    }),
    { messages: 0, calls: 0, errors: 0 },
  )
}
