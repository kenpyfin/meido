import { ScrollArea } from '@/components/ui/scroll-area'
import React, { useMemo } from 'react'

/** Matches `initial_llm_request_v1` from the gateway snapshot. */
export type InitialLlmRequestV1 = {
  schema?: string
  system_prompt?: string
  tool_names_first_turn?: string[]
  messages?: Array<{ role?: string; content?: unknown }>
}

function formatMessageContent(content: unknown): string {
  if (content === null || content === undefined) return ''
  if (typeof content === 'string') return content
  if (Array.isArray(content)) {
    const parts: string[] = []
    for (const block of content) {
      if (!block || typeof block !== 'object') continue
      const b = block as Record<string, unknown>
      const t = b.type
      if (t === 'text' && typeof b.text === 'string') {
        parts.push(b.text)
      } else if (t === 'image_omitted') {
        const mt = typeof b.media_type === 'string' ? b.media_type : 'image'
        const chars =
          typeof b.approx_base64_chars === 'number' ? b.approx_base64_chars : '?'
        parts.push(`[image: ${mt}, ~${chars} base64 chars omitted from snapshot]`)
      } else if (t === 'tool_use') {
        const name = typeof b.name === 'string' ? b.name : 'tool'
        const input =
          b.input !== undefined ? JSON.stringify(b.input, null, 2) : '{}'
        parts.push(`[tool_use: ${name}]\n${input}`)
      } else if (t === 'tool_result') {
        const id = typeof b.tool_use_id === 'string' ? b.tool_use_id : '?'
        const body = typeof b.content === 'string' ? b.content : JSON.stringify(b.content)
        parts.push(`[tool_result: ${id}]\n${body}`)
      } else {
        parts.push(JSON.stringify(block, null, 2))
      }
    }
    return parts.join('\n\n')
  }
  try {
    return JSON.stringify(content, null, 2)
  } catch {
    return String(content)
  }
}

/** Human-readable label for synthetic / special user messages in the snapshot. */
export function messageSectionLabel(body: string, role: string): string | null {
  if (role !== 'user') return null
  if (body.includes('[persona_context]')) {
    return 'persona context - Tier 2/3, operator focus, bookmarks'
  }
  if (body.includes('[system_runtime_context]')) {
    return 'runtime context (date/time)'
  }
  if (body.includes('[scheduler_policy]')) {
    return 'scheduler policy'
  }
  if (body.includes('<user_message')) {
    return 'chat (user)'
  }
  return null
}

/** Full transcript of the messages array as sent on the first LLM call. */
export function formatMessagesTranscript(
  messages: InitialLlmRequestV1['messages'],
): string {
  if (!messages?.length) {
    return '(no messages in snapshot)'
  }
  const blocks: string[] = []
  for (let i = 0; i < messages.length; i++) {
    const m = messages[i]!
    const role =
      typeof m.role === 'string' && m.role.trim() ? m.role.trim().toLowerCase() : 'unknown'
    const body = formatMessageContent(m.content).trim()
    const section = messageSectionLabel(body, role)
    const header = section
      ? `── Message ${i + 1} · ${role} · ${section} ──`
      : `── Message ${i + 1} · ${role} ──`
    blocks.push(`${header}\n\n${body || '(empty)'}`)
  }
  return blocks.join('\n\n')
}

export function snapshotHasPersonaContext(messages: InitialLlmRequestV1['messages']): boolean {
  if (!messages?.length) return false
  return messages.some((m) => {
    const body = formatMessageContent(m.content)
    return body.includes('[persona_context]')
  })
}

type Props = {
  jsonText: string
  appearance: 'dark' | 'light'
}

/**
 * Renders the first-turn LLM snapshot: system, tools, and messages as one formatted transcript.
 */
export function InitialRunPromptView({ jsonText }: Props) {
  const parsed = useMemo((): InitialLlmRequestV1 | null => {
    try {
      const v = JSON.parse(jsonText) as unknown
      return v && typeof v === 'object' ? (v as InitialLlmRequestV1) : null
    } catch {
      return null
    }
  }, [jsonText])

  const messagesTranscript = useMemo(
    () => (parsed ? formatMessagesTranscript(parsed.messages) : ''),
    [parsed],
  )

  const hasPersonaContext = useMemo(
    () => (parsed ? snapshotHasPersonaContext(parsed.messages) : false),
    [parsed],
  )

  const panel =
    'rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-surface-elevated)]'

  const preBody =
    'whitespace-pre-wrap break-words p-3 font-mono text-[12px] leading-relaxed text-[color:var(--mc-text-primary)]'

  if (!parsed || typeof parsed !== 'object') {
    return (
      <div className={panel}>
        <pre className={`max-h-[min(72vh,560px)] overflow-auto ${preBody}`}>{jsonText}</pre>
      </div>
    )
  }

  const tools = Array.isArray(parsed.tool_names_first_turn) ? parsed.tool_names_first_turn : []
  const messageCount = Array.isArray(parsed.messages) ? parsed.messages.length : 0

  return (
    <ScrollArea className="max-h-[min(72vh,560px)] w-full">
      <div className="flex flex-col gap-4 pr-3 pb-1">
        {parsed.schema ? (
          <span>
            Schema: <code className="text-xs">{parsed.schema}</code>
          </span>
        ) : null}

        {typeof parsed.system_prompt === 'string' ? (
          <section>
            <span className="text-lg font-semibold mb-2 block tracking-tight">
              System prompt
            </span>
            <div className={panel}>
              <pre className={preBody}>{parsed.system_prompt}</pre>
            </div>
          </section>
        ) : null}

        <section>
          <span className="text-lg font-semibold mb-2 block tracking-tight">
            Tools (first turn)
          </span>
          {tools.length > 0 ? (
            <div className={`flex flex-wrap gap-2 ${panel} p-3`}>
              {tools.map((name) => (
                <code
                  key={name}
                  className="rounded border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] px-2 py-1 text-[11px] text-[color:var(--mc-text-primary)]"
                >
                  {name}
                </code>
              ))}
            </div>
          ) : (
            <span className={`block ${panel} p-3`}>
              None - conversational turn (no tools on first call).
            </span>
          )}
        </section>

        <section>
          <div className="flex flex-col gap-1 mb-2">
            <span className="text-lg font-semibold block tracking-tight">
              Messages
            </span>
            <span>
              {messageCount} message{messageCount === 1 ? '' : 's'} sent to the model on the first
              call (full text below). Identity and Tier 1 are in the system prompt above.
              Tier 2/3 memory, operator memo, and bookmarks appear in the message labeled{' '}
              <code className="text-xs">persona context</code> when included.
            </span>
            {!hasPersonaContext && messageCount > 0 ? (
              <span className="text-amber-600 dark:text-amber-500">
                No <code className="text-xs">[persona_context]</code> block in this snapshot - memory
                / memo / bookmarks were empty or removed before the LLM call (e.g. token trim).
              </span>
            ) : null}
          </div>
          <div className={panel}>
            <pre className={`max-h-[min(52vh,480px)] overflow-auto ${preBody}`}>
              {messagesTranscript}
            </pre>
          </div>
        </section>
      </div>
    </ScrollArea>
  )
}
