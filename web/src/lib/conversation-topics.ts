export type TopicSideChat = {
  id: string
  title: string
  anchor_message_id: string
  turn_count: number
}

export type ConversationTopic = {
  id: string
  session_id: string | null
  parent_id: string | null
  continues_topic_id: string | null
  title: string
  summary: string
  start_message_id: string
  end_message_id: string
  start_ts: string
  end_ts: string
  message_count: number
  status: 'open' | 'closed' | string
  source: 'auto' | 'user' | string
  pinned: boolean
  created_at?: string
  updated_at?: string
  side_chats: TopicSideChat[]
}

export type TopicsResponse = {
  ok: boolean
  segmenting: boolean
  unsegmented_count: number
  topics: ConversationTopic[]
}

export function conversationTopicsKey(personaId: number | null, sessionId: string | null) {
  return ['conversationTopics', personaId, sessionId ?? ''] as const
}

/** Last topic whose range contains `iso`. Parsed instants, so millisecond formatting does not matter. */
export function topicIdForTimestamp(
  topics: Array<Pick<ConversationTopic, 'id' | 'start_ts' | 'end_ts'>>,
  iso: string,
): string | null {
  const time = Date.parse(iso)
  if (Number.isNaN(time)) return null
  let match: string | null = null
  for (const topic of topics) {
    const start = Date.parse(topic.start_ts)
    const end = Date.parse(topic.end_ts)
    if (Number.isNaN(start) || Number.isNaN(end)) continue
    if (time >= start && time <= end) match = topic.id
  }
  return match
}

/** Newest start first. Ties break on id so the order matches the stored chronological list reversed. */
export function sortTopicsByTimeDesc(topics: ConversationTopic[]): ConversationTopic[] {
  return [...topics].sort((a, b) => {
    if (a.start_ts !== b.start_ts) return a.start_ts < b.start_ts ? 1 : -1
    if (a.id !== b.id) return a.id < b.id ? 1 : -1
    return 0
  })
}

/** True when another topic in the same list starts earlier, so merge-with-previous has a target. */
export function topicHasEarlierNeighbor(topics: ConversationTopic[], topicId: string): boolean {
  const topic = topics.find((entry) => entry.id === topicId)
  if (!topic) return false
  return topics.some(
    (other) =>
      other.id !== topic.id &&
      (other.start_ts < topic.start_ts ||
        (other.start_ts === topic.start_ts && other.id < topic.id)),
  )
}

export function filterTopicTree(topics: ConversationTopic[], query: string): ConversationTopic[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return topics
  const out: ConversationTopic[] = []
  for (const topic of topics) {
    const titleMatch = topic.title.toLowerCase().includes(needle)
    const sideChats = titleMatch
      ? topic.side_chats
      : topic.side_chats.filter((side) => sideLabel(side).toLowerCase().includes(needle))
    if (titleMatch || sideChats.length > 0) {
      out.push(titleMatch ? topic : { ...topic, side_chats: sideChats })
    }
  }
  return out
}

export function sideLabel(side: Pick<TopicSideChat, 'title'>): string {
  const title = side.title.trim()
  return title || 'Side chat'
}

export function formatTopicWhen(iso: string, now = Date.now()): string {
  const time = Date.parse(iso)
  if (Number.isNaN(time)) return ''
  const delta = Math.max(0, now - time)
  const minute = 60_000
  const hour = 60 * minute
  const day = 24 * hour
  if (delta < hour) return `${Math.max(1, Math.round(delta / minute))}m`
  if (delta < day) return `${Math.round(delta / hour)}h`
  if (delta < 14 * day) return `${Math.round(delta / day)}d`
  return iso.slice(0, 10)
}
