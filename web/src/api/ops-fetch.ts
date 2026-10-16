import { api } from './client'
import type { BackgroundJobItem, Persona, QueueLane } from '../types'

export async function fetchQueueLanesForChat(chatId: number): Promise<QueueLane[]> {
  const data = await api<import('../types').QueueDiagnosticsResponse>('/api/queue_diagnostics')
  const lanes = Array.isArray(data.lanes) ? data.lanes : []
  return lanes.filter((l) => l.chat_id === chatId)
}

export function pickQueueLaneForPersona(
  lanes: QueueLane[],
  personaId: number | null,
): QueueLane | null {
  if (personaId != null && personaId > 0) {
    const match = lanes.find((l) => l.persona_id === personaId)
    if (match) return match
  }
  return lanes[0] ?? null
}

export async function fetchQueueLaneForPersona(
  chatId: number,
  personaId: number | null,
): Promise<QueueLane | null> {
  const lanes = await fetchQueueLanesForChat(chatId)
  return pickQueueLaneForPersona(lanes, personaId)
}

/** @deprecated Use fetchQueueLaneForPersona */
export async function fetchQueueLaneForChat(chatId: number): Promise<QueueLane | null> {
  const lanes = await fetchQueueLanesForChat(chatId)
  return lanes[0] ?? null
}

export async function fetchBackgroundLaneForChat(
  chatId: number,
): Promise<BackgroundJobItem[]> {
  const data = await api<import('../types').QueueDiagnosticsResponse>('/api/queue_diagnostics')
  const map = data.background_by_chat
  if (!map || typeof map !== 'object') return []
  const key = String(chatId)
  const items = map[key]
  return Array.isArray(items) ? items : []
}

export type BackgroundJobsSnapshot = {
  jobs: BackgroundJobItem[]
  activeCount: number
}

export async function fetchBackgroundJobsSnapshot(chatId: number): Promise<BackgroundJobsSnapshot> {
  const q = new URLSearchParams({ chat_id: String(chatId) })
  const data = await api<{ jobs?: BackgroundJobItem[]; active_heartbeats?: unknown[]; active_count?: number }>(
    `/api/background_jobs?${q.toString()}`,
  )
  const jobs: BackgroundJobItem[] = Array.isArray(data.jobs) ? data.jobs : []
  const activeCountFromApi = typeof data.active_count === 'number' && Number.isFinite(data.active_count)
    ? Math.max(0, Math.floor(data.active_count))
    : null
  const activeByStatus = jobs.filter((j) =>
    ['pending', 'running', 'completed_raw', 'main_agent_processing'].includes(j.status),
  ).length
  return { jobs, activeCount: activeCountFromApi ?? activeByStatus }
}

export type PersonaApiRow = {
  id: number
  name: string
  is_active: boolean
  /** Latest user or agent message, including sessions and side chats. */
  last_message_at?: string | null
  last_bot_message_at?: string | null
  last_bot_message_session_id?: string | null
  last_bot_message_session_title?: string | null
  last_read_at?: string | null
  agent_engine_override?: string | null
  agent_engine_effective?: string
}

export function mapPersonaApiRow(p: PersonaApiRow): Persona {
  return {
    id: p.id,
    name: p.name,
    is_active: p.is_active,
    last_message_at: p.last_message_at ?? null,
    last_bot_message_at: p.last_bot_message_at ?? null,
    last_bot_message_session_id: p.last_bot_message_session_id ?? null,
    last_bot_message_session_title: p.last_bot_message_session_title ?? null,
    last_read_at: p.last_read_at ?? null,
    agent_engine_override: p.agent_engine_override,
    agent_engine_effective: p.agent_engine_effective,
  }
}

export async function fetchPersonasSnapshot(chatId: number): Promise<Persona[]> {
  const query = new URLSearchParams({ chat_id: String(chatId) })
  const data = await api<{ personas?: PersonaApiRow[] }>(`/api/personas?${query.toString()}`)
  const list = Array.isArray(data.personas) ? data.personas : []
  return list.map(mapPersonaApiRow)
}

export type OpsCollabCounts = {
  unreadRuns: number
  blockedItems: number
}

export type OpsPollBundle = {
  queueLanes: QueueLane[]
  backgroundActiveCount: number
  backgroundJobs: BackgroundJobItem[]
  personasSnapshot: Persona[]
  /** False when this tick skipped the personas HTTP call (caller should keep prior snapshot). */
  personasIncluded: boolean
  collab: OpsCollabCounts
}

export type OpsPollApiResponse = {
  lanes?: QueueLane[]
  jobs?: BackgroundJobItem[]
  active_count?: number
  personas_included?: boolean
  personas?: PersonaApiRow[]
  collab?: { unread_runs?: number; blocked_items?: number }
}

export async function fetchOpsPollBundle(
  chatId: number,
  opts?: { includePersonas?: boolean },
): Promise<OpsPollBundle> {
  const includePersonas = opts?.includePersonas !== false
  const q = new URLSearchParams({ chat_id: String(chatId) })
  if (!includePersonas) q.set('include_personas', '0')
  const data = await api<OpsPollApiResponse>(`/api/ops_poll?${q.toString()}`)
  const jobs: BackgroundJobItem[] = Array.isArray(data.jobs) ? data.jobs : []
  const activeCountFromApi =
    typeof data.active_count === 'number' && Number.isFinite(data.active_count)
      ? Math.max(0, Math.floor(data.active_count))
      : null
  const activeByStatus = jobs.filter((j) =>
    ['pending', 'running', 'completed_raw', 'main_agent_processing'].includes(j.status),
  ).length
  const personasIncluded = data.personas_included !== false
  const list = Array.isArray(data.personas) ? data.personas : []
  return {
    queueLanes: Array.isArray(data.lanes) ? data.lanes : [],
    backgroundActiveCount: activeCountFromApi ?? activeByStatus,
    backgroundJobs: jobs,
    personasSnapshot: list.map(mapPersonaApiRow),
    personasIncluded,
    collab: {
      unreadRuns: data.collab?.unread_runs ?? 0,
      blockedItems: data.collab?.blocked_items ?? 0,
    },
  }
}

export function sumPendingOnOtherPersonas(
  lanes: QueueLane[],
  activePersonaId: number | null,
): number {
  return lanes
    .filter((l) => activePersonaId == null || l.persona_id !== activePersonaId)
    .reduce((sum, l) => sum + (l.pending ?? 0), 0)
}
