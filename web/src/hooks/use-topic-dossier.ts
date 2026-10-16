import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { api } from '../api/client'
import {
  dossierCursorKey,
  TOPIC_DOSSIER_LIMIT,
  type DossierCursor,
  type TopicDossier,
} from '../lib/topic-focus'

export function topicDossierKey(
  personaId: number | null,
  sessionId: string | null,
  topicId: string | null,
  cursor: DossierCursor,
) {
  return ['topicDossier', personaId, sessionId ?? '', topicId ?? '', dossierCursorKey(cursor)] as const
}

function dossierPath(
  personaId: number,
  sessionId: string | null,
  topicId: string,
  cursor: DossierCursor,
): string {
  const params = new URLSearchParams({ limit: String(TOPIC_DOSSIER_LIMIT) })
  if (sessionId) params.set('session_id', sessionId)
  if (cursor.kind === 'before') params.set('before_id', cursor.messageId)
  if (cursor.kind === 'after') params.set('after_id', cursor.messageId)
  if (cursor.kind === 'around') params.set('around_id', cursor.messageId)
  return `/api/personas/${personaId}/topics/${encodeURIComponent(topicId)}/dossier?${params}`
}

export function useTopicDossier(
  personaId: number | null,
  sessionId: string | null,
  topicId: string | null,
  cursor: DossierCursor,
  enabled: boolean,
) {
  const queryClient = useQueryClient()
  const cursorKey = dossierCursorKey(cursor)
  const query = useQuery({
    queryKey: topicDossierKey(personaId, sessionId, topicId, cursor),
    enabled: enabled && personaId != null && topicId != null,
    staleTime: 15_000,
    queryFn: ({ signal }) =>
      api<TopicDossier>(
        dossierPath(personaId as number, sessionId, topicId as string, cursor),
        { signal },
      ),
  })

  useEffect(() => {
    if (!enabled || personaId == null || topicId == null) return
    queryClient.removeQueries({
      predicate: (entry) => {
        const key = entry.queryKey
        if (key[0] !== 'topicDossier' || key[1] !== personaId) return false
        const sameTopic = key[3] === topicId
        const samePage = key[4] === cursorKey
        return !sameTopic || !samePage
      },
    })
  }, [cursorKey, enabled, personaId, queryClient, topicId])

  return query
}
