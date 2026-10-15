import { describe, expect, it } from 'vitest'
import {
  filterTopicTree,
  sortTopicsByTimeDesc,
  topicHasEarlierNeighbor,
  topicIdForTimestamp,
  type ConversationTopic,
} from './conversation-topics'

function topic(partial: Partial<ConversationTopic> & Pick<ConversationTopic, 'id' | 'title' | 'start_ts' | 'end_ts'>): ConversationTopic {
  return {
    session_id: null,
    parent_id: null,
    continues_topic_id: null,
    summary: '',
    start_message_id: `${partial.id}-start`,
    end_message_id: `${partial.id}-end`,
    message_count: 1,
    status: 'closed',
    source: 'auto',
    pinned: false,
    side_chats: [],
    ...partial,
  }
}

describe('topicIdForTimestamp', () => {
  const topics = [
    topic({ id: 'a', title: 'A', start_ts: '2026-04-01T00:00:00Z', end_ts: '2026-04-01T01:00:00Z' }),
    topic({ id: 'b', title: 'B', start_ts: '2026-04-01T02:00:00Z', end_ts: '2026-04-01T03:00:00Z' }),
  ]

  it('maps a message instant onto the containing topic', () => {
    expect(topicIdForTimestamp(topics, '2026-04-01T02:30:00.000Z')).toBe('b')
    expect(topicIdForTimestamp(topics, '2026-04-01T01:30:00Z')).toBeNull()
  })

  it('prefers the later topic when ranges share an instant', () => {
    const overlapping = [
      topic({ id: 'a', title: 'A', start_ts: '2026-04-01T00:00:00Z', end_ts: '2026-04-01T02:00:00Z' }),
      topic({ id: 'b', title: 'B', start_ts: '2026-04-01T02:00:00Z', end_ts: '2026-04-01T03:00:00Z' }),
    ]
    expect(topicIdForTimestamp(overlapping, '2026-04-01T02:00:00Z')).toBe('b')
  })
})

describe('sortTopicsByTimeDesc', () => {
  it('lists the latest start first and keeps an earlier neighbor', () => {
    const topics = [
      topic({ id: 'a', title: 'A', start_ts: '2026-04-01T00:00:00Z', end_ts: '2026-04-01T01:00:00Z' }),
      topic({ id: 'b', title: 'B', start_ts: '2026-04-02T00:00:00Z', end_ts: '2026-04-02T01:00:00Z' }),
      topic({ id: 'c', title: 'C', start_ts: '2026-04-02T00:00:00Z', end_ts: '2026-04-02T02:00:00Z' }),
    ]
    expect(sortTopicsByTimeDesc(topics).map((entry) => entry.id)).toEqual(['c', 'b', 'a'])
    expect(topicHasEarlierNeighbor(topics, 'a')).toBe(false)
    expect(topicHasEarlierNeighbor(topics, 'b')).toBe(true)
    expect(topicHasEarlierNeighbor(sortTopicsByTimeDesc(topics), 'c')).toBe(true)
  })
})

describe('filterTopicTree', () => {
  const topics = [
    topic({
      id: 'a',
      title: 'Deploy logs',
      start_ts: '2026-04-01T00:00:00Z',
      end_ts: '2026-04-01T01:00:00Z',
      side_chats: [{ id: 's1', title: 'Rollback', anchor_message_id: 'm', turn_count: 2 }],
    }),
    topic({ id: 'b', title: 'Budget', start_ts: '2026-04-02T00:00:00Z', end_ts: '2026-04-02T01:00:00Z' }),
  ]

  it('matches topic titles and keeps side chats', () => {
    const found = filterTopicTree(topics, 'deploy')
    expect(found.map((t) => t.id)).toEqual(['a'])
    expect(found[0].side_chats).toHaveLength(1)
  })

  it('keeps a topic when only a side chat title matches', () => {
    const found = filterTopicTree(topics, 'roll')
    expect(found.map((t) => t.id)).toEqual(['a'])
    expect(found[0].side_chats.map((s) => s.id)).toEqual(['s1'])
  })
})
