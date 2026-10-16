import React, { useCallback, useEffect, useRef, useState } from 'react'
import { api, makeHeaders } from '../api/client'
import type {
  BackendMessage,
  PersonaBulletinHistorySuffix,
  SideChatTurn,
} from '../types'
import { ConfirmDialog } from './confirm-dialog'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { MarkdownStream } from '@/components/markdown-stream'
import { makeReplySnippet } from '../lib/reply-quote'

export type SubthreadSidePaneProps = {
  chatId: number | null
  personaId: number | null
  sessionId: string | null
  sideChatId: string
  anchorMessageId: string
  anchorMessage: BackendMessage
  historySuffix: PersonaBulletinHistorySuffix | null
  turns: SideChatTurn[]
  onTurnsChange: React.Dispatch<React.SetStateAction<SideChatTurn[]>>
  draft: string
  onDraftChange: (draft: string) => void
  onDraftLocalChange?: (draft: string) => void
  onMessageActivity?: () => void
  onSendComplete?: () => void | Promise<void>
  onAddToMainChat?: (turnId: string) => void | Promise<void>
  onDelete?: () => void | Promise<boolean>
  autoFocusComposer?: boolean
  onClose: () => void
}

type SseEvent = { event: string; data: string; id?: string }

async function* parseSseEvents(resp: Response): AsyncGenerator<SseEvent, void, unknown> {
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

function parseJsonObject(raw: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>
    if (parsed && typeof parsed === 'object') return parsed
    return null
  } catch {
    return null
  }
}

/** Optimistic send() ids are `a-${Date.now()}` / `u-${Date.now()}`; DB turns use UUIDs. */
function isPersistedTurnId(id: string): boolean {
  return !id.startsWith('a-') && !id.startsWith('u-')
}

export function SubthreadSidePane({
  chatId,
  personaId,
  sessionId,
  sideChatId,
  anchorMessageId,
  anchorMessage,
  historySuffix,
  turns,
  onTurnsChange,
  draft,
  onDraftChange,
  onDraftLocalChange,
  onMessageActivity,
  onSendComplete,
  onAddToMainChat,
  onDelete,
  autoFocusComposer = true,
  onClose,
}: SubthreadSidePaneProps) {
  const [status, setStatus] = useState('Ready')
  const [error, setError] = useState('')
  const [sending, setSending] = useState(false)
  const [promotingTurnId, setPromotingTurnId] = useState<string | null>(null)
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [contextWindow, setContextWindow] = useState<{
    min_user: number
    min_assistant: number
  } | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)
  const inputRef = useRef<HTMLTextAreaElement | null>(null)
  const closeWrapRef = useRef<HTMLDivElement | null>(null)
  const draftPersistTimer = useRef<number | null>(null)

  const anchorSnippet = makeReplySnippet(
    typeof anchorMessage.content === 'string' ? anchorMessage.content : '',
  )

  useEffect(() => {
    if (autoFocusComposer) inputRef.current?.focus()
    else closeWrapRef.current?.querySelector('button')?.focus()
  }, [autoFocusComposer, sideChatId, anchorMessageId])

  useEffect(() => {
    const el = listRef.current
    if (!el) return
    el.scrollTop = el.scrollHeight
  }, [turns, sending])

  useEffect(() => {
    return () => {
      abortRef.current?.abort()
      if (draftPersistTimer.current != null) {
        window.clearTimeout(draftPersistTimer.current)
      }
    }
  }, [])

  const windowLabel = (() => {
    if (contextWindow) {
      return `${contextWindow.min_user} user / ${contextWindow.min_assistant} assistant`
    }
    if (historySuffix) {
      return `${historySuffix.min_user.effective} user / ${historySuffix.min_assistant.effective} assistant`
    }
    return 'cockpit window'
  })()

  const handleDraftInput = useCallback(
    (value: string) => {
      onDraftLocalChange?.(value)
      if (draftPersistTimer.current != null) {
        window.clearTimeout(draftPersistTimer.current)
      }
      draftPersistTimer.current = window.setTimeout(() => {
        onDraftChange(value)
      }, 400)
    },
    [onDraftChange, onDraftLocalChange],
  )

  const send = useCallback(async () => {
    const text = draft.trim()
    if (!text || sending || chatId == null || personaId == null) return

    const userTurn: SideChatTurn = {
      id: `u-${Date.now()}`,
      role: 'user',
      content: text,
    }
    const assistantId = `a-${Date.now()}`
    const historyPayload = turns.map((t) => ({
      role: t.role === 'assistant' ? 'assistant' : 'user',
      content: t.content,
    }))

    onDraftLocalChange?.('')
    onDraftChange('')
    setError('')
    setSending(true)
    setStatus('Sending…')
    onTurnsChange((prev) => [
      ...prev,
      userTurn,
      { id: assistantId, role: 'assistant', content: '', streaming: true },
    ])

    const abort = new AbortController()
    abortRef.current = abort

    try {
      const sendResponse = await api<{
        run_id?: string
        context_window?: { min_user?: number; min_assistant?: number }
      }>('/api/subthread_stream', {
        method: 'POST',
        body: JSON.stringify({
          chat_id: chatId,
          persona_id: personaId,
          session_id: sessionId,
          side_chat_id: sideChatId,
          anchor_message_id: anchorMessageId,
          message: text,
          history: historyPayload,
        }),
        signal: abort.signal,
      })

      const runId = sendResponse.run_id
      if (!runId) throw new Error('missing run_id')

      if (
        sendResponse.context_window &&
        typeof sendResponse.context_window.min_user === 'number' &&
        typeof sendResponse.context_window.min_assistant === 'number'
      ) {
        setContextWindow({
          min_user: sendResponse.context_window.min_user,
          min_assistant: sendResponse.context_window.min_assistant,
        })
      }

      setStatus('Queued')
      const sseResp = await fetch(`/api/stream?run_id=${encodeURIComponent(runId)}`, {
        headers: makeHeaders(),
        signal: abort.signal,
      })
      if (!sseResp.ok) {
        throw new Error(`stream subscribe failed (HTTP ${sseResp.status})`)
      }

      let completed = false
      for await (const evt of parseSseEvents(sseResp)) {
        if (abort.signal.aborted) break
        if (evt.event === 'status') {
          const obj = parseJsonObject(evt.data)
          const message = typeof obj?.message === 'string' ? obj.message : null
          if (message) setStatus(message)
          continue
        }
        if (evt.event === 'delta') {
          const obj = parseJsonObject(evt.data)
          const delta = typeof obj?.delta === 'string' ? obj.delta : ''
          if (!delta) continue
          onTurnsChange((prev) =>
            prev.map((t) =>
              t.id === assistantId ? { ...t, content: `${t.content}${delta}`, streaming: true } : t,
            ),
          )
          continue
        }
        if (evt.event === 'done') {
          const obj = parseJsonObject(evt.data)
          const response = typeof obj?.response === 'string' ? obj.response : ''
          onTurnsChange((prev) =>
            prev.map((t) =>
              t.id === assistantId
                ? {
                    ...t,
                    content: response || t.content,
                    streaming: false,
                  }
                : t,
            ),
          )
          completed = true
          setStatus('Idle')
          break
        }
        if (evt.event === 'error') {
          const obj = parseJsonObject(evt.data)
          const err = typeof obj?.error === 'string' ? obj.error : 'Side chat failed'
          throw new Error(err)
        }
      }

      if (!completed && !abort.signal.aborted) {
        onTurnsChange((prev) =>
          prev.map((t) => (t.id === assistantId ? { ...t, streaming: false } : t)),
        )
        setStatus('Idle')
      }
      if (completed) {
        onMessageActivity?.()
        await onSendComplete?.()
      }
    } catch (e) {
      if (abort.signal.aborted) {
        setStatus('Cancelled')
        onTurnsChange((prev) =>
          prev.map((t) =>
            t.id === assistantId
              ? { ...t, content: t.content || '(cancelled)', streaming: false }
              : t,
          ),
        )
      } else {
        const msg = e instanceof Error ? e.message : String(e)
        setError(msg)
        setStatus('Error')
        onTurnsChange((prev) =>
          prev.map((t) =>
            t.id === assistantId
              ? { ...t, content: t.content || `Error: ${msg}`, streaming: false }
              : t,
          ),
        )
      }
    } finally {
      setSending(false)
      abortRef.current = null
    }
  }, [
    anchorMessageId,
    chatId,
    draft,
    onDraftChange,
    onDraftLocalChange,
    onMessageActivity,
    onSendComplete,
    onTurnsChange,
    personaId,
    sending,
    sessionId,
    sideChatId,
    turns,
  ])

  const cancel = useCallback(() => {
    abortRef.current?.abort()
  }, [])

  const handleConfirmDelete = useCallback(async () => {
    if (!onDelete) return
    setDeleting(true)
    try {
      const ok = await onDelete()
      if (ok) setDeleteConfirmOpen(false)
    } finally {
      setDeleting(false)
    }
  }, [onDelete])

  const handleAddToMainChat = useCallback(
    async (turnId: string) => {
      if (!onAddToMainChat || promotingTurnId || sending) return
      setError('')
      setPromotingTurnId(turnId)
      setStatus('Adding to main chat…')
      try {
        await onAddToMainChat(turnId)
        setStatus('Added to main chat')
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setError(msg)
        setStatus('Error')
      } finally {
        setPromotingTurnId(null)
      }
    },
    [onAddToMainChat, promotingTurnId, sending],
  )

  return (
    <aside className="mc-subthread-pane" aria-label="Side chat">
      <header className="mc-subthread-header">
        <div className="mc-subthread-header-text">
          <div className="mc-subthread-title">Side chat</div>
          <div className="mc-subthread-meta">Saved · context {windowLabel}</div>
        </div>
        <div className="mc-subthread-header-actions">
          {onDelete ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="mc-subthread-delete h-8"
              onClick={() => setDeleteConfirmOpen(true)}
              aria-label="Delete side chat"
              title="Delete side chat"
              disabled={sending}
            >
              Delete
            </Button>
          ) : null}
          <div ref={closeWrapRef}>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="mc-subthread-close"
            onClick={onClose}
            aria-label="Close side chat"
            title="Close"
          >
            ×
          </Button>
          </div>
        </div>
      </header>

      <div className="mc-subthread-anchor" role="note">
        <div className="mc-subthread-anchor-label">Anchored to assistant reply</div>
        <div className="mc-subthread-anchor-snippet">{anchorSnippet}</div>
      </div>

      <div className="mc-subthread-list" ref={listRef}>
        {turns.length === 0 ? (
          <div className="mc-subthread-empty">
            Ask a follow-up about this reply. Side-chat turns stay out of the main timeline.
          </div>
        ) : (
          turns.map((turn) => {
            const canPromote =
              turn.role === 'assistant' &&
              !turn.streaming &&
              !!turn.content.trim() &&
              isPersistedTurnId(turn.id) &&
              !!onAddToMainChat
            const promoting = promotingTurnId === turn.id
            return (
              <div
                key={turn.id}
                className={
                  turn.role === 'user' ? 'mc-subthread-bubble mc-subthread-bubble-user' : 'mc-subthread-bubble'
                }
              >
                <div className="mc-subthread-bubble-role flex flex-wrap items-center gap-2">
                  <span>{turn.role === 'user' ? 'You' : 'Assistant'}</span>
                  {turn.streaming ? (
                    <Badge variant="secondary" className="text-[10px]">
                      streaming
                    </Badge>
                  ) : null}
                </div>
                {turn.role === 'assistant' ? (
                  <div className="mc-subthread-markdown">
                    <MarkdownStream streaming={!!turn.streaming}>
                      {turn.content || (turn.streaming ? '…' : '')}
                    </MarkdownStream>
                  </div>
                ) : (
                  <div className="mc-subthread-plain">{turn.content}</div>
                )}
                {canPromote ? (
                  <div className="mc-subthread-bubble-actions">
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      className="mc-subthread-promote"
                      onClick={() => void handleAddToMainChat(turn.id)}
                      disabled={promoting || sending || promotingTurnId != null}
                      title="Add this reply to the main chat"
                      aria-label="Add to main chat"
                    >
                      {promoting ? 'Adding…' : 'Add to main chat'}
                    </Button>
                  </div>
                ) : null}
              </div>
            )
          })
        )}
      </div>

      {error ? <div className="mc-subthread-error">{error}</div> : null}

      <div className="mc-subthread-composer">
        <textarea
          ref={inputRef}
          className="mc-subthread-input"
          value={draft}
          rows={3}
          placeholder="Ask about this reply…"
          disabled={sending}
          onChange={(e) => handleDraftInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void send()
            }
          }}
        />
        <div className="mc-subthread-composer-actions">
          <span className="mc-subthread-status">{status}</span>
          {sending ? (
            <Button type="button" variant="outline" size="sm" className="mc-subthread-send" onClick={cancel}>
              Stop
            </Button>
          ) : (
            <Button
              type="button"
              size="sm"
              className="mc-subthread-send mc-subthread-send-primary"
              onClick={() => void send()}
              disabled={!draft.trim() || chatId == null || personaId == null}
            >
              Send
            </Button>
          )}
        </div>
      </div>

      <ConfirmDialog
        open={deleteConfirmOpen}
        onOpenChange={setDeleteConfirmOpen}
        title="Delete side chat?"
        description="This removes this side conversation and all of its turns. The main chat timeline is unchanged."
        confirmLabel="Delete"
        destructive
        loading={deleting}
        onConfirm={() => void handleConfirmDelete()}
      />
    </aside>
  )
}
