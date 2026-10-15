import { useCallback, useEffect, useRef, useState } from 'react'
import type { ThreadMessageLike } from '@assistant-ui/react'
import { api } from '../api/client'
import { historiesEqual, mapBackendHistory } from '../lib/history-sync'
import { formatReplyForSend, makeReplySnippet, type PendingReplyQuote } from '../lib/reply-quote'
import { HISTORY_PAGE_SIZE } from '../app/constants'
import type { BackendMessage, PersonaMessageBookmark } from '../types'
import type { PendingConfirm } from '../components/confirm-dialog'

export type HistoryWindowMode = 'live' | 'anchored'

export type UseChatHistoryOptions = {
  chatId: number | null
  activePersonaId: number | null
  activeSessionId: string | null
  setError: (message: string) => void
  setStatusText: (message: string) => void
  requestConfirm: (opts: PendingConfirm) => void
  setPersonaBookmarks: React.Dispatch<React.SetStateAction<PersonaMessageBookmark[]>>
  hideScheduled?: boolean
}

type HistoryApiResponse = {
  messages?: BackendMessage[]
  has_older?: boolean
  has_newer?: boolean
}

function messageIdOf(m: ThreadMessageLike): string {
  return typeof m.id === 'string' ? m.id : ''
}

/** Merge by id preserving chronological order; prefer `incoming` content on conflict. */
function mergeById(
  existing: ThreadMessageLike[],
  incoming: ThreadMessageLike[],
): ThreadMessageLike[] {
  const map = new Map<string, ThreadMessageLike>()
  for (const m of existing) {
    const id = messageIdOf(m)
    if (id) map.set(id, m)
  }
  for (const m of incoming) {
    const id = messageIdOf(m)
    if (id) map.set(id, m)
  }
  const ids = new Set(map.keys())
  // Preserve relative order: walk existing then append unknown from incoming in incoming order.
  const out: ThreadMessageLike[] = []
  for (const m of existing) {
    const id = messageIdOf(m)
    if (!id || !ids.has(id)) continue
    out.push(map.get(id)!)
    ids.delete(id)
  }
  for (const m of incoming) {
    const id = messageIdOf(m)
    if (!id || !ids.has(id)) continue
    out.push(map.get(id)!)
    ids.delete(id)
  }
  return out
}

export function useChatHistory({
  chatId,
  activePersonaId,
  activeSessionId,
  setError,
  setStatusText,
  requestConfirm,
  setPersonaBookmarks,
  hideScheduled = false,
}: UseChatHistoryOptions) {
  const [historySeed, setHistorySeed] = useState<ThreadMessageLike[]>([])
  const [historyByDay, setHistoryByDay] = useState<Record<string, ThreadMessageLike[]>>({})
  const [historyMode, setHistoryMode] = useState<HistoryWindowMode>('live')
  const [historyHasOlder, setHistoryHasOlder] = useState(false)
  const [historyHasNewer, setHistoryHasNewer] = useState(false)
  const [newMessagesPending, setNewMessagesPending] = useState(false)
  const [historyLoadingMore, setHistoryLoadingMore] = useState(false)
  const [historyLoadingNewer, setHistoryLoadingNewer] = useState(false)
  const [historyLoading, setHistoryLoading] = useState(true)
  const [draftByThreadKey, setDraftByThreadKey] = useState<Record<string, string>>({})
  const [pendingReplyByThreadKey, setPendingReplyByThreadKey] = useState<
    Record<string, PendingReplyQuote>
  >({})

  const activeSessionIdRef = useRef<string | null>(null)
  activeSessionIdRef.current = activeSessionId
  const hideScheduledRef = useRef(hideScheduled)
  hideScheduledRef.current = hideScheduled
  const historySeedRef = useRef<ThreadMessageLike[]>([])
  const historyModeRef = useRef<HistoryWindowMode>(historyMode)
  const pendingReplyRef = useRef<PendingReplyQuote | null>(null)

  useEffect(() => {
    historySeedRef.current = historySeed
  }, [historySeed])
  useEffect(() => {
    historyModeRef.current = historyMode
  }, [historyMode])
  useEffect(() => {
    const threadKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
    pendingReplyRef.current = pendingReplyByThreadKey[threadKey] ?? null
  }, [chatId, activePersonaId, pendingReplyByThreadKey])

  const resetHistoryPagination = useCallback(() => {
    setHistoryMode('live')
    setHistoryHasOlder(false)
    setHistoryHasNewer(false)
    setNewMessagesPending(false)
    setHistorySeed([])
    historySeedRef.current = []
  }, [])

  const applyLiveWindow = useCallback((mapped: ThreadMessageLike[], hasOlder: boolean) => {
    setHistoryByDay({})
    setHistoryMode('live')
    setHistoryHasOlder(hasOlder)
    setHistoryHasNewer(false)
    setNewMessagesPending(false)
    if (!historiesEqual(historySeedRef.current, mapped)) {
      setHistorySeed(mapped)
    }
  }, [])

  const applyAnchoredWindow = useCallback(
    (mapped: ThreadMessageLike[], hasOlder: boolean, hasNewer: boolean) => {
      setHistoryByDay({})
      setHistoryMode('anchored')
      setHistoryHasOlder(hasOlder)
      setHistoryHasNewer(hasNewer)
      // Stay pending until Jump to latest; do not clear here.
      if (!historiesEqual(historySeedRef.current, mapped)) {
        setHistorySeed(mapped)
      }
    },
    [],
  )

  const loadHistory = useCallback(
    async (
      cid: number | null = chatId,
      personaId?: number | null,
      day?: string | null,
      opts?: {
        force?: boolean
        limitOverride?: number
        sessionId?: string | null
        aroundId?: string
        aroundBefore?: number
        aroundAfter?: number
        beforeId?: string
        afterId?: string
        /** Replace seed instead of merging (session switch / jump to latest). */
        replace?: boolean
      },
    ): Promise<void> => {
      if (cid == null) return
      const pid =
        personaId != null && personaId > 0
          ? personaId
          : activePersonaId != null && activePersonaId > 0
            ? activePersonaId
            : null
      if (pid == null) {
        setHistorySeed([])
        setHistoryByDay({})
        setHistoryHasOlder(false)
        setHistoryHasNewer(false)
        setHistoryMode('live')
        setNewMessagesPending(false)
        return
      }

      const isCursor =
        Boolean(opts?.aroundId) || Boolean(opts?.beforeId) || Boolean(opts?.afterId) || Boolean(day)
      const mode = historyModeRef.current

      // Background poll / run-complete: never overwrite an anchored window unless
      // this call is an explicit cursor/replace navigation.
      if (!isCursor && !opts?.replace && mode === 'anchored') {
        try {
          const query = new URLSearchParams({
            chat_id: String(cid),
            persona_id: String(pid),
            limit: '1',
          })
          const sid = opts?.sessionId !== undefined ? opts.sessionId : activeSessionIdRef.current
          if (sid) query.set('session_id', sid)
          if (hideScheduledRef.current) query.set('hide_scheduled', '1')
          const data = await api<HistoryApiResponse>(`/api/history?${query.toString()}`)
          const raw = Array.isArray(data.messages) ? data.messages : []
          const newestId = raw.length > 0 ? raw[raw.length - 1]?.id ?? '' : ''
          const seed = historySeedRef.current
          const seedNewest = seed.length > 0 ? messageIdOf(seed[seed.length - 1]!) : ''
          if (newestId && newestId !== seedNewest) {
            setNewMessagesPending(true)
          }
        } catch {
          // Ignore poll failures while anchored.
        }
        return
      }

      const query = new URLSearchParams({ chat_id: String(cid), persona_id: String(pid) })
      const sid = opts?.sessionId !== undefined ? opts.sessionId : activeSessionIdRef.current
      if (sid) query.set('session_id', sid)
      if (hideScheduledRef.current) query.set('hide_scheduled', '1')

      if (opts?.aroundId) {
        query.set('around_id', opts.aroundId)
        query.set('around_before', String(opts.aroundBefore ?? 4))
        query.set('around_after', String(opts.aroundAfter ?? 5))
      } else if (opts?.beforeId) {
        query.set('before_id', opts.beforeId)
        query.set('limit', String(opts.limitOverride ?? HISTORY_PAGE_SIZE))
      } else if (opts?.afterId) {
        query.set('after_id', opts.afterId)
        query.set('limit', String(opts.limitOverride ?? HISTORY_PAGE_SIZE))
      } else if (day) {
        query.set('day', day)
      } else {
        query.set('limit', String(opts?.limitOverride ?? HISTORY_PAGE_SIZE))
      }

      const data = await api<HistoryApiResponse>(`/api/history?${query.toString()}`)
      const rawMessages = Array.isArray(data.messages) ? data.messages : []
      const mapped = mapBackendHistory(rawMessages)
      const hasOlder = Boolean(data.has_older)
      const hasNewer = Boolean(data.has_newer)

      if (opts?.aroundId) {
        applyAnchoredWindow(mapped, hasOlder, hasNewer)
        return
      }
      if (opts?.beforeId) {
        const merged = mergeById(mapped, historySeedRef.current)
        setHistoryHasOlder(hasOlder)
        if (!historiesEqual(historySeedRef.current, merged)) {
          setHistorySeed(merged)
        }
        return
      }
      if (opts?.afterId) {
        const merged = mergeById(historySeedRef.current, mapped)
        if (!hasNewer) {
          setHistoryMode('live')
          setHistoryHasNewer(false)
          setNewMessagesPending(false)
        } else {
          setHistoryHasNewer(true)
        }
        // Loading newer does not change has_older of the top of the window.
        if (!historiesEqual(historySeedRef.current, merged)) {
          setHistorySeed(merged)
        }
        return
      }
      if (day) {
        setHistoryByDay((prev) => {
          const nextByDay = { ...prev, [day]: mapped }
          const allDays = Object.keys(nextByDay).sort()
          const combined = allDays.flatMap((d) => nextByDay[d] ?? [])
          setHistoryMode('live')
          setHistoryHasOlder(false)
          setHistoryHasNewer(false)
          setNewMessagesPending(false)
          if (!historiesEqual(historySeedRef.current, combined)) {
            setHistorySeed(combined)
          }
          return nextByDay
        })
        return
      }

      // Live / replace path.
      if (opts?.replace || historySeedRef.current.length === 0 || mode === 'live') {
        const pageSize = opts?.limitOverride ?? HISTORY_PAGE_SIZE
        const stillOnePage = historySeedRef.current.length <= pageSize
        if (opts?.replace || historySeedRef.current.length === 0 || stillOnePage) {
          applyLiveWindow(mapped, hasOlder)
        } else {
          // Merge so pages loaded via Load earlier are kept when polling the tip.
          const merged = mergeById(historySeedRef.current, mapped)
          // Tip refresh: has_older stays if we already had older pages, or from response
          // when the seed was still a single live page.
          const seedHadOlderThanPage =
            historySeedRef.current.length > mapped.length || hasOlder
          setHistoryByDay({})
          setHistoryMode('live')
          setHistoryHasOlder(seedHadOlderThanPage || hasOlder)
          setHistoryHasNewer(false)
          setNewMessagesPending(false)
          if (!historiesEqual(historySeedRef.current, merged)) {
            setHistorySeed(merged)
          }
        }
      }
    },
    [activePersonaId, applyAnchoredWindow, applyLiveWindow, chatId],
  )

  const hideScheduledPrev = useRef(hideScheduled)
  useEffect(() => {
    if (hideScheduledPrev.current === hideScheduled) return
    hideScheduledPrev.current = hideScheduled
    if (chatId == null) return
    void loadHistory(chatId, activePersonaId, null, { force: true, replace: true })
  }, [activePersonaId, chatId, hideScheduled, loadHistory])

  const loadOlder = useCallback(async () => {
    if (chatId == null) return
    if (historyLoadingMore) return
    const seed = historySeedRef.current
    const firstId = seed.length > 0 ? messageIdOf(seed[0]!) : ''
    if (!firstId) return
    setHistoryLoadingMore(true)
    try {
      await loadHistory(chatId, activePersonaId ?? undefined, null, {
        force: true,
        beforeId: firstId,
      })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setHistoryLoadingMore(false)
    }
  }, [activePersonaId, chatId, historyLoadingMore, loadHistory, setError])

  const loadNewer = useCallback(async () => {
    if (chatId == null) return
    if (historyLoadingNewer) return
    const seed = historySeedRef.current
    const lastId = seed.length > 0 ? messageIdOf(seed[seed.length - 1]!) : ''
    if (!lastId) return
    setHistoryLoadingNewer(true)
    try {
      await loadHistory(chatId, activePersonaId ?? undefined, null, {
        force: true,
        afterId: lastId,
      })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setHistoryLoadingNewer(false)
    }
  }, [activePersonaId, chatId, historyLoadingNewer, loadHistory, setError])

  const jumpToLatest = useCallback(async () => {
    if (chatId == null) return
    try {
      setStatusText('Jumping to latest…')
      await loadHistory(chatId, activePersonaId ?? undefined, null, {
        force: true,
        replace: true,
        limitOverride: HISTORY_PAGE_SIZE,
      })
      setStatusText('Idle')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setStatusText('Idle')
    }
  }, [activePersonaId, chatId, loadHistory, setError, setStatusText])

  /** Load a light window around a message (bookmark / side-chat jump). */
  const loadWindowAround = useCallback(
    async (
      messageId: string,
      sessionId?: string | null,
    ): Promise<{ ok: boolean; sessionId: string | null }> => {
      if (chatId == null || activePersonaId == null) {
        return { ok: false, sessionId: null }
      }
      const alreadyVisible = historySeedRef.current.some((m) => messageIdOf(m) === messageId)
      if (alreadyVisible) {
        return {
          ok: true,
          sessionId:
            sessionId !== undefined
              ? sessionId
              : activeSessionIdRef.current,
        }
      }
      try {
        let resolvedSession =
          sessionId !== undefined ? sessionId : activeSessionIdRef.current
        if (sessionId === undefined) {
          const data = await api<{
            message?: BackendMessage
          }>(`/api/personas/${activePersonaId}/messages/${encodeURIComponent(messageId)}`)
          const m = data.message
          if (!m?.id) return { ok: false, sessionId: null }
          resolvedSession =
            typeof m.session_id === 'string' && m.session_id.trim()
              ? m.session_id.trim()
              : null
        }
        await loadHistory(chatId, activePersonaId, null, {
          force: true,
          replace: true,
          sessionId: resolvedSession,
          aroundId: messageId,
          aroundBefore: 4,
          aroundAfter: 5,
        })
        return { ok: true, sessionId: resolvedSession }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        return { ok: false, sessionId: null }
      }
    },
    [activePersonaId, chatId, loadHistory, setError],
  )

  const handleReplyToMessage = useCallback(
    async (messageId: string) => {
      if (activePersonaId == null) return
      try {
        setStatusText('Loading quote…')
        const data = await api<{ message?: BackendMessage }>(
          `/api/personas/${activePersonaId}/messages/${encodeURIComponent(messageId)}`,
        )
        const m = data.message
        const raw = typeof m?.content === 'string' ? m.content.trim() : ''
        if (!raw) {
          setError('Cannot reply: message has no text content')
          setStatusText('Idle')
          return
        }
        const threadKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
        const quote: PendingReplyQuote = {
          messageId,
          snippet: makeReplySnippet(raw),
          fullContent: raw,
          senderName: typeof m?.sender_name === 'string' ? m.sender_name : '',
          isFromBot: Boolean(m?.is_from_bot),
        }
        setPendingReplyByThreadKey((prev) => ({ ...prev, [threadKey]: quote }))
        pendingReplyRef.current = quote
        setStatusText('Quote ready — add your reply')
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        setStatusText('Idle')
      }
    },
    [activePersonaId, chatId, setError, setStatusText],
  )

  const handleDismissPendingReply = useCallback(() => {
    const threadKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
    setPendingReplyByThreadKey((prev) => {
      if (!prev[threadKey]) return prev
      const next = { ...prev }
      delete next[threadKey]
      return next
    })
    pendingReplyRef.current = null
  }, [activePersonaId, chatId])

  const handleDeleteMessage = useCallback(
    (messageId: string) => {
      if (activePersonaId == null) return
      requestConfirm({
        title: 'Delete message',
        description:
          'This permanently removes the message from the database. Bookmarks for this message are also removed.',
        confirmLabel: 'Delete message',
        destructive: true,
        onConfirm: async () => {
          try {
            await api(`/api/personas/${activePersonaId}/messages/${encodeURIComponent(messageId)}`, {
              method: 'DELETE',
            })
            setHistorySeed((prev) => prev.filter((m) => m.id !== messageId))
            setPersonaBookmarks((prev) => prev.filter((b) => b.message_id !== messageId))
            setStatusText('Message deleted')
            setError('')
          } catch (e) {
            setError(e instanceof Error ? e.message : String(e))
            throw e
          }
        },
      })
    },
    [activePersonaId, requestConfirm, setError, setPersonaBookmarks, setStatusText],
  )

  const handleSaveMessageEdit = useCallback(
    async (messageId: string, content: string) => {
      if (activePersonaId == null) return
      const trimmed = content.trim()
      if (!trimmed) {
        setError('Edited message cannot be empty')
        return
      }
      try {
        setStatusText('Saving edit…')
        await api(`/api/personas/${activePersonaId}/messages/${encodeURIComponent(messageId)}`, {
          method: 'PATCH',
          body: JSON.stringify({ content: trimmed }),
        })
        setHistorySeed((prev) =>
          prev.map((m) => {
            if (m.id !== messageId) return m
            return {
              ...m,
              content: [{ type: 'text' as const, text: trimmed }],
            }
          }),
        )
        setPersonaBookmarks((prev) =>
          prev.map((b) => {
            if (b.message_id !== messageId) return b
            const preview =
              Array.from(trimmed).length > 280
                ? `${Array.from(trimmed).slice(0, 280).join('')}…`
                : trimmed
            return { ...b, content_preview: preview }
          }),
        )
        setStatusText('Message updated')
        setError('')
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        setStatusText('Idle')
        throw e
      }
    },
    [activePersonaId, setError, setPersonaBookmarks, setStatusText],
  )

  const activeDraftKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
  const activeDraftText = draftByThreadKey[activeDraftKey] ?? ''
  const activePendingReply = pendingReplyByThreadKey[activeDraftKey] ?? null

  const handleDraftTextChange = useCallback(
    (nextText: string) => {
      setDraftByThreadKey((prev) => {
        const current = prev[activeDraftKey] ?? ''
        if (current === nextText) return prev
        return { ...prev, [activeDraftKey]: nextText }
      })
    },
    [activeDraftKey],
  )

  return {
    historySeed,
    setHistorySeed,
    historyByDay,
    historyMode,
    historyHasOlder,
    historyHasNewer,
    newMessagesPending,
    historyLoadingMore,
    historyLoadingNewer,
    historyLoading,
    setHistoryLoading,
    loadHistory,
    loadOlder,
    loadNewer,
    jumpToLatest,
    loadWindowAround,
    /** @deprecated Prefer loadOlder */
    loadMoreHistory: loadOlder,
    /** @deprecated Prefer historyHasOlder */
    historyHasMore: historyHasOlder,
    /** @deprecated Prefer loadWindowAround */
    ensureMessageVisible: loadWindowAround,
    resetHistoryPagination,
    handleReplyToMessage,
    handleDismissPendingReply,
    handleDeleteMessage,
    handleSaveMessageEdit,
    activeDraftText,
    activePendingReply,
    handleDraftTextChange,
    pendingReplyRef,
    formatReplyForSend,
    setDraftByThreadKey,
    setPendingReplyByThreadKey,
  }
}
