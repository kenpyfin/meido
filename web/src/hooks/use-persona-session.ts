import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { api } from '../api/client'
import { mapPersonaApiRow, type PersonaApiRow } from '../api/ops-fetch'
import { HISTORY_PAGE_SIZE } from '../app/constants'
import {
  clearPersonaLastReadLocal,
  readPersonaLastReadAt,
  readStoredPersonaId,
  resolveStoredSessionId,
  toMs,
  writeStoredPersonaId,
  writeStoredSessionForPersona,
} from '../lib/persona-storage'
import { type EngineId } from '../lib/agent-engine'
import type { ChatSession, Persona } from '../types'
import type { PendingConfirm } from '../components/confirm-dialog'

export type UsePersonaSessionOptions = {
  chatId: number | null
  setHistoryLoading: (loading: boolean) => void
  loadHistory: (
    cid?: number | null,
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
      replace?: boolean
    },
  ) => Promise<void>
  resetHistoryPagination: () => void
  loadPersonaBulletin: (pid: number) => Promise<void>
  requestConfirm: (opts: PendingConfirm) => void
  setError: (message: string) => void
  setStatusText: (message: string) => void
  onPersonaLoaded?: (personaId: number | null) => void
}

export function usePersonaSession({
  chatId,
  setHistoryLoading,
  loadHistory,
  resetHistoryPagination,
  loadPersonaBulletin,
  requestConfirm,
  setError,
  setStatusText,
  onPersonaLoaded,
}: UsePersonaSessionOptions) {
  const [personas, setPersonas] = useState<Persona[]>([])
  const [activePersonaId, setActivePersonaId] = useState<number | null>(null)
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null)
  const [chatSessions, setChatSessions] = useState<ChatSession[]>([])
  const [newSchedulePersonaId, setNewSchedulePersonaId] = useState<number | null>(null)
  const readCursorInflightRef = useRef<Set<string>>(new Set())

  const activeSessionIdRef = useRef<string | null>(null)
  activeSessionIdRef.current = activeSessionId

  const activePersonaName = personas.find((p) => p.id === activePersonaId)?.name ?? null

  const personaHasNew = useMemo<Record<number, boolean>>(() => {
    if (chatId == null) return {}
    const out: Record<number, boolean> = {}
    for (const p of personas) {
      if (p.id === activePersonaId) {
        out[p.id] = false
        continue
      }
      const lastBotMs = toMs(p.last_bot_message_at ?? null)
      if (lastBotMs == null) {
        out[p.id] = false
        continue
      }
      const lastReadMs = toMs(p.last_read_at ?? readPersonaLastReadAt(chatId, p.id))
      out[p.id] = lastReadMs == null ? true : lastBotMs > lastReadMs
    }
    return out
  }, [chatId, personas, activePersonaId])

  const syncMissingReadCursors = useCallback(
    async (
      cid: number,
      rows: { id: number; last_bot_message_at?: string | null; last_read_at?: string | null }[],
    ) => {
      const missing = rows.filter((p) => p.last_read_at == null)
      if (missing.length === 0) return
      const items = missing.map((p) => ({
        persona_id: p.id,
        last_read_at:
          readPersonaLastReadAt(cid, p.id)
          ?? p.last_bot_message_at
          ?? new Date().toISOString(),
      }))
      const data = await api<{ cursors?: { persona_id: number; last_read_at: string }[] }>(
        '/api/personas/read',
        {
          method: 'POST',
          body: JSON.stringify({ chat_id: cid, if_absent: true, items }),
        },
      )
      const byId = new Map((data.cursors ?? []).map((c) => [c.persona_id, c.last_read_at]))
      setPersonas((prev) =>
        prev.map((p) => {
          const stamped = byId.get(p.id)
          if (!stamped) return p
          return { ...p, last_read_at: stamped }
        }),
      )
      clearPersonaLastReadLocal(cid, missing.map((p) => p.id))
    },
    [],
  )

  const markPersonaRead = useCallback(
    (personaId: number, chatIdOverride?: number | null) => {
      const cid = chatIdOverride ?? chatId
      if (cid == null) return
      const optimistic = new Date().toISOString()
      setPersonas((prev) =>
        prev.map((p) => (p.id === personaId ? { ...p, last_read_at: optimistic } : p)),
      )
      void api<{ cursors?: { persona_id: number; last_read_at: string }[] }>('/api/personas/read', {
        method: 'POST',
        body: JSON.stringify({
          chat_id: cid,
          if_absent: false,
          items: [{ persona_id: personaId, last_read_at: optimistic }],
        }),
      })
        .then((data) => {
          const stamped = data.cursors?.find((c) => c.persona_id === personaId)?.last_read_at
          if (!stamped) return
          setPersonas((prev) =>
            prev.map((p) => (p.id === personaId ? { ...p, last_read_at: stamped } : p)),
          )
        })
        .catch(() => {
          // Optimistic cursor stays until the next personas poll.
        })
    },
    [chatId],
  )

  const loadPersonas = useCallback(
    async (cid: number | null = chatId): Promise<{ id: number; name: string } | null> => {
      if (cid == null) return null
      try {
        const query = new URLSearchParams({ chat_id: String(cid) })
        const data = await api<{ personas?: PersonaApiRow[] }>(`/api/personas?${query.toString()}`)
        const list = Array.isArray(data.personas) ? data.personas : []
        const personaList = list.map(mapPersonaApiRow)
        const missingRead = personaList.filter((p) => p.last_read_at == null)
        const seeded = personaList.map((p) => {
          if (p.last_read_at != null) return p
          return {
            ...p,
            last_read_at:
              readPersonaLastReadAt(cid, p.id)
              ?? p.last_bot_message_at
              ?? new Date().toISOString(),
          }
        })
        setPersonas(seeded)
        if (missingRead.length > 0) {
          void syncMissingReadCursors(cid, missingRead).catch(() => {
            // Poll or the next personas load retries while cursors are still absent.
          })
        }
        const active = list.find((p) => p.is_active)
        const defaultChoice = active ?? list[0]
        const storedId = readStoredPersonaId()
        const storedInList = storedId !== null && list.some((p) => p.id === storedId)
        const chosen = storedInList && list.find((p) => p.id === storedId)
          ? { id: list.find((p) => p.id === storedId)!.id, name: list.find((p) => p.id === storedId)!.name }
          : defaultChoice
            ? { id: defaultChoice.id, name: defaultChoice.name }
            : null
        if (chosen) {
          setActivePersonaId(chosen.id)
          setNewSchedulePersonaId((prev) => prev ?? chosen.id)
          if (!storedInList) writeStoredPersonaId(chosen.id)
        } else {
          setActivePersonaId(null)
        }
        onPersonaLoaded?.(chosen?.id ?? null)
        return chosen
      } catch {
        setPersonas([])
        setActivePersonaId(null)
        return null
      }
    },
    [chatId, onPersonaLoaded, syncMissingReadCursors],
  )

  useEffect(() => {
    if (chatId == null) return
    const missing = personas.filter((p) => p.last_read_at == null)
    if (missing.length === 0) return
    const key = `${chatId}:${missing.map((p) => p.id).sort((a, b) => a - b).join(',')}`
    if (readCursorInflightRef.current.has(key)) return
    readCursorInflightRef.current.add(key)
    void syncMissingReadCursors(chatId, missing)
      .catch(() => {
        readCursorInflightRef.current.delete(key)
      })
  }, [chatId, personas, syncMissingReadCursors])

  const loadSessions = useCallback(
    async (cid?: number | null, personaId?: number | null): Promise<ChatSession[]> => {
      const c = cid ?? chatId
      const p = personaId ?? activePersonaId
      if (c == null || p == null) {
        setChatSessions([])
        return []
      }
      try {
        const data = await api<{ sessions?: ChatSession[] }>(
          `/api/chat_sessions?chat_id=${c}&persona_id=${p}`,
        )
        const sessions = Array.isArray(data.sessions) ? data.sessions : []
        setChatSessions(sessions)
        return sessions
      } catch {
        setChatSessions([])
        return []
      }
    },
    [activePersonaId, chatId],
  )

  const switchPersona = useCallback(
    async (
      personaName: string,
      opts?: { sessionId?: string | null },
    ): Promise<void> => {
      if (chatId == null) return
      setHistoryLoading(true)
      try {
        await api('/api/personas/switch', {
          method: 'POST',
          body: JSON.stringify({ chat_id: chatId, persona_name: personaName }),
        })
        const p = personas.find((x) => x.name === personaName)
        if (p) writeStoredPersonaId(p.id)
        await loadPersonas(chatId)
        const sessions = await loadSessions(chatId, p?.id)
        let targetSessionId: string | null
        if (opts !== undefined && 'sessionId' in opts) {
          const want = opts.sessionId ?? null
          if (want == null) {
            targetSessionId = null
          } else {
            const match = sessions.find((s) => s.id === want)
            targetSessionId = match ? match.id : null
          }
        } else {
          targetSessionId = p ? resolveStoredSessionId(sessions, p.id) : null
        }
        setActiveSessionId(targetSessionId)
        if (p) writeStoredSessionForPersona(p.id, targetSessionId)
        resetHistoryPagination()
        await loadHistory(chatId, p?.id ?? undefined, null, {
          force: true,
          limitOverride: HISTORY_PAGE_SIZE,
          sessionId: targetSessionId,
        })
        if (p) markPersonaRead(p.id)
        if (p) await loadPersonaBulletin(p.id)
      } finally {
        setHistoryLoading(false)
      }
    },
    [
      chatId,
      loadHistory,
      loadPersonaBulletin,
      loadPersonas,
      loadSessions,
      markPersonaRead,
      personas,
      resetHistoryPagination,
      setHistoryLoading,
    ],
  )

  const handleSelectSession = useCallback(
    async (sessionId: string | null) => {
      setHistoryLoading(true)
      try {
        setActiveSessionId(sessionId)
        if (activePersonaId != null) {
          writeStoredSessionForPersona(activePersonaId, sessionId)
        }
        resetHistoryPagination()
        await loadHistory(chatId, activePersonaId ?? undefined, null, {
          force: true,
          limitOverride: HISTORY_PAGE_SIZE,
          sessionId,
        })
      } finally {
        setHistoryLoading(false)
      }
    },
    [activePersonaId, chatId, loadHistory, resetHistoryPagination, setHistoryLoading],
  )

  const handleCreateSession = useCallback(
    async (
      intent: string,
      mirrorMainChat = false,
      options?: { sourceTopicId?: string },
    ): Promise<boolean> => {
      if (chatId == null || activePersonaId == null) return false
      setHistoryLoading(true)
      try {
        const data = await api<{
          session?: ChatSession
          session_id?: string
          title?: string
        }>('/api/chat_sessions', {
          method: 'POST',
          body: JSON.stringify({
            chat_id: chatId,
            persona_id: activePersonaId,
            intent,
            mirror_main_chat: options?.sourceTopicId ? false : mirrorMainChat,
            ...(options?.sourceTopicId ? { source_topic_id: options.sourceTopicId } : {}),
          }),
        })
        const session =
          data.session ??
          (data.session_id
            ? {
                id: data.session_id,
                chat_id: chatId,
                persona_id: activePersonaId,
                title: data.title ?? intent.slice(0, 60),
                intent,
                status: 'active' as const,
                created_at: new Date().toISOString(),
                last_active_at: new Date().toISOString(),
                ttl_hours: 0,
                mirror_main_chat: options?.sourceTopicId ? false : mirrorMainChat,
                source_topic_id: options?.sourceTopicId ?? null,
              }
            : null)
        if (!session) return false

        setChatSessions((prev) => {
          if (prev.some((s) => s.id === session.id)) return prev
          return [session, ...prev]
        })
        setActiveSessionId(session.id)
        writeStoredSessionForPersona(activePersonaId, session.id)
        resetHistoryPagination()
        await loadHistory(chatId, activePersonaId, null, {
          force: true,
          limitOverride: HISTORY_PAGE_SIZE,
          sessionId: session.id,
        })
        void loadSessions()
        return true
      } finally {
        setHistoryLoading(false)
      }
    },
    [activePersonaId, chatId, loadHistory, loadSessions, resetHistoryPagination, setHistoryLoading],
  )

  const handleDeleteSession = useCallback(
    async (sessionId: string) => {
      await api(`/api/chat_sessions/${encodeURIComponent(sessionId)}`, { method: 'DELETE' })
      if (activeSessionId === sessionId) {
        setHistoryLoading(true)
        try {
          setActiveSessionId(null)
          if (activePersonaId != null) {
            writeStoredSessionForPersona(activePersonaId, null)
          }
          resetHistoryPagination()
          await loadHistory(chatId, activePersonaId ?? undefined, null, {
            force: true,
            limitOverride: HISTORY_PAGE_SIZE,
            sessionId: null,
          })
        } finally {
          setHistoryLoading(false)
        }
      }
      await loadSessions()
    },
    [
      activePersonaId,
      activeSessionId,
      chatId,
      loadHistory,
      loadSessions,
      resetHistoryPagination,
      setHistoryLoading,
    ],
  )

  const onCreatePersona = useCallback(
    async (input: { name: string; engine: EngineId; llm_provider?: string; llm_model?: string; llm_base_url?: string; local_base_url?: string; local_model?: string; cursor_sdk_model?: string; gemini_default_model?: string }) => {
      if (chatId == null) return
      const trimmed = input.name.trim()
      if (!trimmed) throw new Error('Persona name cannot be empty')
      await api('/api/personas/create', {
        method: 'POST',
        body: JSON.stringify({
          chat_id: chatId,
          name: trimmed,
          agent_engine: input.engine,
          llm_provider: input.llm_provider,
          llm_model: input.llm_model,
          llm_base_url: input.llm_base_url,
          local_base_url: input.local_base_url,
          local_model: input.local_model,
          cursor_sdk_model: input.cursor_sdk_model,
          gemini_default_model: input.gemini_default_model,
        }),
      })
      await loadPersonas(chatId)
      setStatusText(`Persona "${trimmed}" created`)
      setError('')
    },
    [chatId, loadPersonas, setError, setStatusText],
  )

  const onRenamePersona = useCallback(
    async (personaId: number, name: string) => {
      if (chatId == null) return
      const trimmed = name.trim()
      if (!trimmed) throw new Error('Persona name cannot be empty')
      await api('/api/personas/rename', {
        method: 'POST',
        body: JSON.stringify({ chat_id: chatId, persona_id: personaId, name: trimmed }),
      })
      await loadPersonas(chatId)
      setStatusText(`Persona renamed to "${trimmed}"`)
      setError('')
    },
    [chatId, loadPersonas, setError, setStatusText],
  )

  const onDeletePersona = useCallback(
    (personaId: number) => {
      if (chatId == null) return
      const personaName = personas.find((p) => p.id === personaId)?.name ?? 'this persona'
      requestConfirm({
        title: 'Delete persona',
        description: `Delete "${personaName}"? Its messages and session will be removed permanently.`,
        confirmLabel: 'Delete persona',
        destructive: true,
        onConfirm: async () => {
          try {
            await api('/api/personas/delete', {
              method: 'POST',
              body: JSON.stringify({ chat_id: chatId, persona_id: personaId }),
            })
            await loadPersonas(chatId)
            if (activePersonaId === personaId) {
              resetHistoryPagination()
              if (activePersonaId != null && activePersonaId > 0) {
                await loadHistory(chatId, activePersonaId, null, {
                  force: true,
                  limitOverride: HISTORY_PAGE_SIZE,
                })
              }
            }
            setStatusText('Persona deleted')
            setError('')
          } catch (e) {
            setError(e instanceof Error ? e.message : String(e))
            throw e
          }
        },
      })
    },
    [
      activePersonaId,
      chatId,
      loadHistory,
      loadPersonas,
      personas,
      requestConfirm,
      resetHistoryPagination,
      setError,
      setStatusText,
    ],
  )

  return {
    personas,
    setPersonas,
    activePersonaId,
    setActivePersonaId,
    activeSessionId,
    setActiveSessionId,
    activeSessionIdRef,
    chatSessions,
    activePersonaName,
    personaHasNew,
    markPersonaRead,
    loadPersonas,
    loadSessions,
    switchPersona,
    handleSelectSession,
    handleCreateSession,
    handleDeleteSession,
    onCreatePersona,
    onRenamePersona,
    onDeletePersona,
    newSchedulePersonaId,
    setNewSchedulePersonaId,
  }
}
