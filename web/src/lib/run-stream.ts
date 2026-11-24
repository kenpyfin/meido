/** Shared SSE helpers for `/api/stream?run_id=…` watchers. */

export type SseEvent = { event: string; data: string; id?: string }

export function parseJsonObject(raw: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>
    if (parsed && typeof parsed === 'object') return parsed
    return null
  } catch {
    return null
  }
}

export async function* parseSseEvents(resp: Response): AsyncGenerator<SseEvent, void, unknown> {
  const body = resp.body
  if (!body) return

  const reader = body.getReader()
  const decoder = new TextDecoder('utf-8')

  let buffer = ''
  let eventName: string | undefined
  let eventId: string | undefined
  let dataLines: string[] = []

  while (true) {
    const { value, done } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })

    while (true) {
      const newlineIdx = buffer.indexOf('\n')
      if (newlineIdx < 0) break

      const line = buffer.slice(0, newlineIdx).replace(/\r$/, '')
      buffer = buffer.slice(newlineIdx + 1)

      if (line === '') {
        if (dataLines.length > 0) {
          yield {
            event: eventName ?? 'message',
            data: dataLines.join('\n'),
            id: eventId,
          }
        }
        eventName = undefined
        eventId = undefined
        dataLines = []
        continue
      }

      if (line.startsWith(':')) continue
      if (line.startsWith('event:')) {
        eventName = line.slice('event:'.length).trim()
        continue
      }
      if (line.startsWith('id:')) {
        eventId = line.slice('id:'.length).trim()
        continue
      }
      if (line.startsWith('data:')) {
        dataLines.push(line.slice('data:'.length).trimStart())
      }
    }
  }

  if (dataLines.length > 0) {
    yield { event: eventName ?? 'message', data: dataLines.join('\n'), id: eventId }
  }
}

export type StreamWatchOutcome = 'done' | 'error' | 'closed' | 'aborted'

export type StreamWatchHandlers = {
  onStatus?: (message: string) => void
  onDelta?: (delta: string) => void
  onMessageStored?: (messageId: string) => void
  onMessageRewritten?: (payload: {
    messageId: string
    original: string
    rewritten: string
  }) => void
  onDone?: (responseText: string) => void
  onError?: (errorText: string, kind?: string) => void
}

/**
 * Consume `/api/stream` until done/error/close. Does not take the ThreadPane abort
 * signal — callers own when to stop yielding vs keep watching in the background.
 */
export async function watchRunStream(
  runId: string,
  opts: {
    headers: HeadersInit
    signal?: AbortSignal
    lastEventId?: string
    handlers?: StreamWatchHandlers
  },
): Promise<StreamWatchOutcome> {
  const params = new URLSearchParams({ run_id: runId })
  if (opts.lastEventId) params.set('last_event_id', opts.lastEventId)
  const sseUrl = `/api/stream?${params.toString()}`
  let resp: Response
  try {
    resp = await fetch(sseUrl, { headers: opts.headers, signal: opts.signal })
  } catch (e) {
    if (opts.signal?.aborted) return 'aborted'
    throw e
  }
  if (!resp.ok) {
    throw new Error(`stream subscribe failed (HTTP ${resp.status})`)
  }

  const handlers = opts.handlers
  try {
    for await (const evt of parseSseEvents(resp)) {
      if (opts.signal?.aborted) return 'aborted'

      if (evt.event === 'message_stored') {
        const obj = parseJsonObject(evt.data)
        if (typeof obj?.message_id === 'string' && obj.message_id) {
          handlers?.onMessageStored?.(obj.message_id)
        }
        continue
      }

      if (evt.event === 'message_rewritten') {
        const obj = parseJsonObject(evt.data)
        const messageId = typeof obj?.message_id === 'string' ? obj.message_id : ''
        const original = typeof obj?.original === 'string' ? obj.original : ''
        const rewritten = typeof obj?.rewritten === 'string' ? obj.rewritten : ''
        if (rewritten) {
          handlers?.onMessageRewritten?.({ messageId, original, rewritten })
        }
        continue
      }

      if (evt.event === 'status') {
        const obj = parseJsonObject(evt.data)
        const message = typeof obj?.message === 'string' ? obj.message : null
        if (message) handlers?.onStatus?.(message)
        continue
      }

      if (evt.event === 'delta') {
        const obj = parseJsonObject(evt.data)
        const delta = typeof obj?.delta === 'string' ? obj.delta : ''
        if (delta) handlers?.onDelta?.(delta)
        continue
      }

      if (evt.event === 'done') {
        const doneObj = parseJsonObject(evt.data)
        const responseText = typeof doneObj?.response === 'string' ? doneObj.response : ''
        handlers?.onDone?.(responseText)
        return 'done'
      }

      if (evt.event === 'error') {
        const obj = parseJsonObject(evt.data)
        const errorText = typeof obj?.error === 'string' ? obj.error : 'unknown error'
        const kind = typeof obj?.kind === 'string' ? obj.kind : undefined
        handlers?.onError?.(errorText, kind)
        return 'error'
      }
    }
  } catch (e) {
    if (opts.signal?.aborted) return 'aborted'
    throw e
  }
  return 'closed'
}
