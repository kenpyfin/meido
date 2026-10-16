import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../api/client'
import { makeReplySnippet } from '../lib/reply-quote'
import type { BackendMessage, SideChatSummary, SideChatTurn } from '../types'

export type ActiveSideChat = {
  id: string
  anchorMessageId: string
  anchorMessage: BackendMessage
  sessionId: string | null
}

export type UseSideChatsOptions = {
  activePersonaId: number | null
  activeSessionId: string | null
  chatId: number | null
  setError: (message: string) => void
  setStatusText: (message: string) => void
}

export function useSideChats({
  activePersonaId,
  activeSessionId,
  chatId,
  setError,
  setStatusText,
}: UseSideChatsOptions) {
  const [sideChats, setSideChats] = useState<SideChatSummary[]>([])
  const [activeSideChat, setActiveSideChat] = useState<ActiveSideChat | null>(null)
  const [activeTurns, setActiveTurns] = useState<SideChatTurn[]>([])
  const [activeDraft, setActiveDraft] = useState('')
  const [sideChatsLoading, setSideChatsLoading] = useState(false)
  const activePersonaIdRef = useRef(activePersonaId)
  activePersonaIdRef.current = activePersonaId

  const loadSideChats = useCallback(async (pid?: number | null) => {
    const personaId = pid ?? activePersonaIdRef.current
    if (personaId == null || personaId <= 0) {
      setSideChats([])
      return
    }
    setSideChatsLoading(true)
    try {
      const data = await api<{ side_chats?: SideChatSummary[] }>(
        `/api/personas/${personaId}/side_chats`,
      )
      setSideChats(Array.isArray(data.side_chats) ? data.side_chats : [])
    } catch {
      setSideChats([])
    } finally {
      setSideChatsLoading(false)
    }
  }, [])

  useEffect(() => {
    setActiveSideChat(null)
    setActiveTurns([])
    setActiveDraft('')
    if (activePersonaId != null && activePersonaId > 0) {
      void loadSideChats(activePersonaId)
    } else {
      setSideChats([])
    }
  }, [activePersonaId, loadSideChats])

  const closeSideChatPane = useCallback(() => {
    setActiveSideChat(null)
  }, [])

  const applyLoadedSideChat = useCallback(
    (
      sideChat: SideChatSummary,
      turns: SideChatTurn[],
      anchorMessage: BackendMessage,
    ) => {
      setActiveSideChat({
        id: sideChat.id,
        anchorMessageId: sideChat.anchor_message_id,
        anchorMessage,
        sessionId: sideChat.session_id ?? null,
      })
      setActiveTurns(turns)
      setActiveDraft(typeof sideChat.draft_text === 'string' ? sideChat.draft_text : '')
      setSideChats((prev) => {
        const without = prev.filter((s) => s.id !== sideChat.id)
        return [sideChat, ...without]
      })
    },
    [],
  )

  const openSideChatForAnchor = useCallback(
    async (messageId: string) => {
      if (activePersonaId == null) return
      if (activeSideChat?.anchorMessageId === messageId) {
        closeSideChatPane()
        return
      }
      try {
        setStatusText('Opening side chat…')
        const msgData = await api<{ message?: BackendMessage }>(
          `/api/personas/${activePersonaId}/messages/${encodeURIComponent(messageId)}`,
        )
        const m = msgData.message
        if (!m || !m.is_from_bot) {
          setError('Side chat requires a bot response')
          setStatusText('Idle')
          return
        }
        const snippet = makeReplySnippet(typeof m.content === 'string' ? m.content : '')
        const data = await api<{
          side_chat?: SideChatSummary
          turns?: SideChatTurn[]
        }>(`/api/personas/${activePersonaId}/side_chats`, {
          method: 'POST',
          body: JSON.stringify({
            anchor_message_id: messageId,
            session_id: activeSessionId,
            anchor_snippet: snippet,
          }),
        })
        if (!data.side_chat) throw new Error('missing side_chat')
        applyLoadedSideChat(data.side_chat, Array.isArray(data.turns) ? data.turns : [], m)
        setStatusText('Side chat open')
        setError('')
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        setStatusText('Idle')
      }
    },
    [
      activePersonaId,
      activeSessionId,
      activeSideChat?.anchorMessageId,
      applyLoadedSideChat,
      closeSideChatPane,
      setError,
      setStatusText,
    ],
  )

  const openSideChatById = useCallback(
    async (sideChatId: string): Promise<SideChatSummary | null> => {
      if (activePersonaId == null) return null
      try {
        setStatusText('Opening side chat…')
        const data = await api<{
          side_chat?: SideChatSummary
          turns?: SideChatTurn[]
        }>(`/api/personas/${activePersonaId}/side_chats/${encodeURIComponent(sideChatId)}`)
        if (!data.side_chat) throw new Error('side chat not found')
        const msgData = await api<{ message?: BackendMessage }>(
          `/api/personas/${activePersonaId}/messages/${encodeURIComponent(data.side_chat.anchor_message_id)}`,
        )
        const m = msgData.message
        if (!m) throw new Error('anchor message not found')
        applyLoadedSideChat(data.side_chat, Array.isArray(data.turns) ? data.turns : [], m)
        setStatusText('Side chat open')
        setError('')
        return data.side_chat
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        setStatusText('Idle')
        return null
      }
    },
    [activePersonaId, applyLoadedSideChat, setError, setStatusText],
  )

  const deleteSideChat = useCallback(
    async (sideChatId: string): Promise<boolean> => {
      if (activePersonaId == null) return false
      try {
        await api(`/api/personas/${activePersonaId}/side_chats/${encodeURIComponent(sideChatId)}`, {
          method: 'DELETE',
        })
        setSideChats((prev) => prev.filter((s) => s.id !== sideChatId))
        if (activeSideChat?.id === sideChatId) {
          setActiveSideChat(null)
          setActiveTurns([])
          setActiveDraft('')
        }
        setStatusText('Side chat deleted')
        return true
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        return false
      }
    },
    [activePersonaId, activeSideChat?.id, setError, setStatusText],
  )

  const persistDraft = useCallback(
    async (draft: string) => {
      if (activePersonaId == null || !activeSideChat) return
      setActiveDraft(draft)
      try {
        await api(
          `/api/personas/${activePersonaId}/side_chats/${encodeURIComponent(activeSideChat.id)}`,
          {
            method: 'PATCH',
            body: JSON.stringify({ draft_text: draft }),
          },
        )
      } catch {
        // Draft sync is best-effort; local state still holds the text.
      }
    },
    [activePersonaId, activeSideChat],
  )

  const refreshAfterSend = useCallback(async () => {
    if (activePersonaId == null || !activeSideChat) return
    try {
      const data = await api<{
        side_chat?: SideChatSummary
        turns?: SideChatTurn[]
      }>(
        `/api/personas/${activePersonaId}/side_chats/${encodeURIComponent(activeSideChat.id)}`,
      )
      if (data.side_chat) {
        setSideChats((prev) => {
          const without = prev.filter((s) => s.id !== data.side_chat!.id)
          return [data.side_chat!, ...without]
        })
      }
      if (Array.isArray(data.turns)) {
        setActiveTurns(data.turns)
      }
    } catch {
      void loadSideChats()
    }
  }, [activePersonaId, activeSideChat, loadSideChats])

  return {
    sideChats,
    sideChatsLoading,
    activeSideChat,
    activeTurns,
    setActiveTurns,
    activeDraft,
    setActiveDraft: persistDraft,
    setActiveDraftLocal: setActiveDraft,
    loadSideChats,
    openSideChatForAnchor,
    openSideChatById,
    closeSideChatPane,
    deleteSideChat,
    refreshAfterSend,
  }
}
