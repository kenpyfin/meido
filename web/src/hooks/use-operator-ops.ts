import { useCallback, useEffect, useState } from 'react'
import { api } from '../api/client'
import { parsePersonaBulletinHistorySuffix } from '../lib/bulletin'
import type {
  PersonaBulletinFocus,
  PersonaBulletinHistorySuffix,
  PersonaDenseDeliveryInfo,
  PersonaMessageBookmark,
} from '../types'

export type UseOperatorOpsOptions = {
  activePersonaId: number | null
  setError: (message: string) => void
  setStatusText: (message: string) => void
}

export function useOperatorOps({
  activePersonaId,
  setError,
  setStatusText,
}: UseOperatorOpsOptions) {
  const [bulletinFocus, setBulletinFocus] = useState<PersonaBulletinFocus | null>(null)
  const [personaBookmarks, setPersonaBookmarks] = useState<PersonaMessageBookmark[]>([])
  const [bulletinHistorySuffix, setBulletinHistorySuffix] =
    useState<PersonaBulletinHistorySuffix | null>(null)
  const [bulletinOperatorMemo, setBulletinOperatorMemo] = useState<string | null>(null)
  const [denseDelivery, setDenseDelivery] = useState<PersonaDenseDeliveryInfo | null>(null)

  const loadPersonaBulletin = useCallback(async (pid: number): Promise<void> => {
    try {
      const data = await api<{
        focus?: PersonaBulletinFocus | null
        bookmarks?: PersonaMessageBookmark[]
        history_suffix?: unknown
        operator_memo?: string | null
        dense_delivery?: PersonaDenseDeliveryInfo | null
        dense_delivery_enabled?: boolean
      }>(`/api/personas/${pid}/bulletin`)
      setBulletinFocus(data.focus ?? null)
      setPersonaBookmarks(Array.isArray(data.bookmarks) ? data.bookmarks : [])
      setBulletinHistorySuffix(parsePersonaBulletinHistorySuffix(data.history_suffix))
      setBulletinOperatorMemo(typeof data.operator_memo === 'string' ? data.operator_memo : null)
      if (data.dense_delivery && typeof data.dense_delivery.enabled === 'boolean') {
        setDenseDelivery(data.dense_delivery)
      } else {
        setDenseDelivery({
          enabled: data.dense_delivery_enabled === true,
          messaging_max_chars: 2000,
          web_max_chars: 1000,
          summary_chars: 800,
        })
      }
    } catch {
      setBulletinFocus(null)
      setPersonaBookmarks([])
      setBulletinHistorySuffix(null)
      setBulletinOperatorMemo(null)
      setDenseDelivery(null)
    }
  }, [])

  const reloadPersonaBulletin = useCallback(async () => {
    if (activePersonaId == null) return
    await loadPersonaBulletin(activePersonaId)
  }, [activePersonaId, loadPersonaBulletin])

  const removePersonaBookmark = useCallback(
    async (messageId: string): Promise<boolean> => {
      if (activePersonaId == null) return false
      try {
        await api(`/api/personas/${activePersonaId}/bookmarks/${encodeURIComponent(messageId)}`, {
          method: 'DELETE',
        })
        setPersonaBookmarks((prev) => prev.filter((b) => b.message_id !== messageId))
        setStatusText('Bookmark removed')
        return true
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        return false
      }
    },
    [activePersonaId, setError, setStatusText],
  )

  useEffect(() => {
    if (activePersonaId != null && activePersonaId > 0) {
      loadPersonaBulletin(activePersonaId).catch(() => {})
    } else {
      setBulletinFocus(null)
      setPersonaBookmarks([])
      setBulletinHistorySuffix(null)
      setBulletinOperatorMemo(null)
      setDenseDelivery(null)
    }
  }, [activePersonaId, loadPersonaBulletin])

  const toggleMessageBookmark = useCallback(
    async (messageId: string, role: 'user' | 'assistant'): Promise<void> => {
      if (activePersonaId == null) return
      const alreadyBookmarked = personaBookmarks.some((b) => b.message_id === messageId)
      try {
        if (alreadyBookmarked) {
          await api(`/api/personas/${activePersonaId}/bookmarks/${encodeURIComponent(messageId)}`, {
            method: 'DELETE',
          })
          setPersonaBookmarks((prev) => prev.filter((b) => b.message_id !== messageId))
          setStatusText('Bookmark removed')
        } else {
          const res = await api<{ bookmark?: PersonaMessageBookmark }>(
            `/api/personas/${activePersonaId}/bookmarks`,
            {
              method: 'POST',
              body: JSON.stringify({ message_id: messageId }),
            },
          )
          const next = res.bookmark
          if (next) {
            setPersonaBookmarks((prev) => [next, ...prev.filter((b) => b.message_id !== messageId)])
          } else {
            await loadPersonaBulletin(activePersonaId)
          }
          setStatusText(`Bookmarked ${role} message`)
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [activePersonaId, loadPersonaBulletin, personaBookmarks, setError, setStatusText],
  )

  return {
    bulletinFocus,
    personaBookmarks,
    setPersonaBookmarks,
    bulletinHistorySuffix,
    bulletinOperatorMemo,
    denseDelivery,
    loadPersonaBulletin,
    reloadPersonaBulletin,
    removePersonaBookmark,
    toggleMessageBookmark,
  }
}
