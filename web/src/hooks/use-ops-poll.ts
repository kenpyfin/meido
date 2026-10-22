import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import type { BackgroundJobItem, Persona, QueueLane } from '../types'
import {
  fetchOpsPollBundle,
  pickQueueLaneForPersona,
  sumPendingOnOtherPersonas,
  type OpsPollBundle,
} from '../api/ops-fetch'

type UseOpsPollArgs = {
  chatId: number | null
  activePersonaId: number | null
  docVisible: boolean
  pendingRunsForActivePersona: number
  setPersonas: React.Dispatch<React.SetStateAction<Persona[]>>
}

/** Compare persona list fields that drive sidebar UI; skip setState when poll data is unchanged. */
export function personasSnapshotEqual(a: Persona[], b: Persona[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) {
    const x = a[i]
    const y = b[i]
    if (
      x.id !== y.id
      || x.name !== y.name
      || x.is_active !== y.is_active
      || (x.last_message_at ?? null) !== (y.last_message_at ?? null)
      || (x.last_bot_message_at ?? null) !== (y.last_bot_message_at ?? null)
      || (x.last_bot_message_session_id ?? null) !== (y.last_bot_message_session_id ?? null)
      || (x.last_bot_message_session_title ?? null) !== (y.last_bot_message_session_title ?? null)
      || (x.last_read_at ?? null) !== (y.last_read_at ?? null)
      || (x.agent_engine_override ?? null) !== (y.agent_engine_override ?? null)
      || (x.agent_engine_effective ?? null) !== (y.agent_engine_effective ?? null)
    ) {
      return false
    }
  }
  return true
}

/**
 * Single combined poll for queue lane, background heartbeats, and persona list refresh.
 * Replaces manual setInterval polling.
 */
export function useOpsPoll({
  chatId,
  activePersonaId,
  docVisible,
  pendingRunsForActivePersona,
  setPersonas,
}: UseOpsPollArgs): {
  queueLane: QueueLane | null
  queueLanesAll: QueueLane[]
  otherPersonasPending: number
  backgroundActiveCount: number
  backgroundJobs: BackgroundJobItem[]
  /** React Query `dataUpdatedAt` for the ops poll snapshot (0 before the first fetch). */
  queueSampledAt: number
  unreadRuns: number
  blockedItems: number
  invalidateOps: (chatIdOverride?: number | null) => Promise<void>
  clearUnreadRuns: () => void
} {
  const queryClient = useQueryClient()
  const lastPersonasRef = useRef<Persona[] | null>(null)
  const lastPersonasFetchMsRef = useRef(0)

  const query = useQuery({
    queryKey: ['opsPoll', chatId],
    queryFn: async (): Promise<OpsPollBundle> => {
      if (chatId == null) {
        throw new Error('opsPoll: missing chatId')
      }
      // Queue/background stay on the fast interval; personas only every 10s (sidebar unread dots).
      const now = Date.now()
      const includePersonas = now - lastPersonasFetchMsRef.current >= 10000
      const bundle = await fetchOpsPollBundle(chatId, { includePersonas })
      if (includePersonas) lastPersonasFetchMsRef.current = now
      return bundle
    },
    enabled: chatId != null,
    // Align with the slowest idle poll so React Query does not mark data stale between ticks.
    staleTime: 2500,
    refetchInterval: (q) => {
      if (chatId == null) return false
      const d = q.state.data
      const lanes = d?.queueLanes ?? []
      const activeLane = pickQueueLaneForPersona(lanes, activePersonaId)
      const qp = (activeLane?.pending ?? 0) > 0
      const otherPending = sumPendingOnOtherPersonas(lanes, activePersonaId) > 0
      const activePending =
        qp
        || otherPending
        || pendingRunsForActivePersona > 0
        || (d?.backgroundActiveCount ?? 0) > 0
      const baseMs = activePending ? 2500 : 10000
      return docVisible ? baseMs : 60000
    },
  })

  useEffect(() => {
    const snap = query.data?.personasSnapshot
    if (!snap || query.data?.personasIncluded === false) return
    if (lastPersonasRef.current && personasSnapshotEqual(lastPersonasRef.current, snap)) {
      return
    }
    lastPersonasRef.current = snap
    setPersonas(snap)
  }, [query.data?.personasSnapshot, query.data?.personasIncluded, setPersonas])

  const queueLanesAll = query.data?.queueLanes ?? []
  const queueLane = useMemo(
    () => pickQueueLaneForPersona(queueLanesAll, activePersonaId),
    [queueLanesAll, activePersonaId],
  )
  const otherPersonasPending = useMemo(
    () => sumPendingOnOtherPersonas(queueLanesAll, activePersonaId),
    [queueLanesAll, activePersonaId],
  )
  const backgroundActiveCount = query.data?.backgroundActiveCount ?? 0
  const backgroundJobs = query.data?.backgroundJobs ?? []

  const invalidateOps = useCallback(
    async (chatIdOverride?: number | null) => {
      const id = chatIdOverride ?? chatId
      if (id == null) return
      lastPersonasFetchMsRef.current = 0
      await queryClient.invalidateQueries({ queryKey: ['opsPoll', id] })
    },
    [chatId, queryClient],
  )

  const clearUnreadRuns = useCallback(() => {
    if (chatId == null) return
    queryClient.setQueryData<OpsPollBundle>(['opsPoll', chatId], (current) => {
      if (!current) return current
      return { ...current, collab: { ...current.collab, unreadRuns: 0 } }
    })
  }, [chatId, queryClient])

  return {
    queueLane,
    queueLanesAll,
    otherPersonasPending,
    backgroundActiveCount,
    backgroundJobs,
    queueSampledAt: query.dataUpdatedAt,
    unreadRuns: query.data?.collab.unreadRuns ?? 0,
    blockedItems: query.data?.collab.blockedItems ?? 0,
    invalidateOps,
    clearUnreadRuns,
  }
}
