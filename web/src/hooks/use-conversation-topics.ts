import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef } from 'react'
import { api } from '../api/client'
import {
  conversationTopicsKey,
  type ConversationTopic,
  type TopicsResponse,
} from '../lib/conversation-topics'

const EMPTY_TOPICS: ConversationTopic[] = []

function topicsPath(personaId: number, sessionId: string | null, suffix = ''): string {
  const params = new URLSearchParams()
  if (sessionId) params.set('session_id', sessionId)
  const query = params.toString()
  return `/api/personas/${personaId}/topics${suffix}${query ? `?${query}` : ''}`
}

export function useConversationTopics(
  personaId: number | null,
  sessionId: string | null,
  turnActive: boolean,
) {
  const queryClient = useQueryClient()
  const turnWasActive = useRef(turnActive)

  const query = useQuery({
    queryKey: conversationTopicsKey(personaId, sessionId),
    enabled: personaId != null,
    queryFn: () => api<TopicsResponse>(topicsPath(personaId as number, sessionId)),
    refetchInterval: (q) => (q.state.data?.segmenting ? 8000 : false),
  })

  useEffect(() => {
    if (turnWasActive.current && !turnActive && personaId != null) {
      void queryClient.invalidateQueries({ queryKey: conversationTopicsKey(personaId, sessionId) })
    }
    turnWasActive.current = turnActive
  }, [personaId, queryClient, sessionId, turnActive])

  const invalidate = useCallback(async () => {
    if (personaId == null) return
    await queryClient.invalidateQueries({ queryKey: conversationTopicsKey(personaId, sessionId) })
  }, [personaId, queryClient, sessionId])

  const rename = useCallback(async (topicId: string, title: string) => {
    if (personaId == null) return
    await api(topicsPath(personaId, sessionId, `/${encodeURIComponent(topicId)}`), {
      method: 'PATCH',
      body: JSON.stringify({ title }),
    })
    await invalidate()
  }, [invalidate, personaId, sessionId])

  const pin = useCallback(async (topicId: string, pinned: boolean) => {
    if (personaId == null) return
    await api(topicsPath(personaId, sessionId, `/${encodeURIComponent(topicId)}`), {
      method: 'PATCH',
      body: JSON.stringify({ pinned }),
    })
    await invalidate()
  }, [invalidate, personaId, sessionId])

  const merge = useCallback(async (topicId: string) => {
    if (personaId == null) return
    await api(topicsPath(personaId, sessionId, `/${encodeURIComponent(topicId)}/merge`), {
      method: 'POST',
    })
    await invalidate()
  }, [invalidate, personaId, sessionId])

  const remove = useCallback(async (topicId: string) => {
    if (personaId == null) return 0
    const res = await api<{ deleted_messages?: number }>(
      topicsPath(personaId, sessionId, `/${encodeURIComponent(topicId)}`),
      { method: 'DELETE' },
    )
    await invalidate()
    await queryClient.invalidateQueries({ queryKey: ['topicDossier', personaId] })
    return typeof res.deleted_messages === 'number' ? res.deleted_messages : 0
  }, [invalidate, personaId, queryClient, sessionId])

  const refresh = useCallback(async () => {
    if (personaId == null) return
    await api(topicsPath(personaId, sessionId, '/refresh'), { method: 'POST' })
    window.setTimeout(() => {
      void invalidate()
    }, 1500)
  }, [invalidate, personaId, sessionId])

  const topics = query.data?.topics ?? EMPTY_TOPICS
  return {
    topics,
    segmenting: query.data?.segmenting ?? false,
    unsegmentedCount: query.data?.unsegmented_count ?? 0,
    loading: query.isLoading,
    error: query.error instanceof Error ? query.error.message : '',
    rename,
    pin,
    merge,
    remove,
    refresh,
    invalidate,
  }
}
