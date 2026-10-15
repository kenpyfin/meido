import type { ThreadMessageLike } from '@assistant-ui/react'
import { describe, expect, it } from 'vitest'
import { historiesEqual, isHistoryPrepend, mapBackendHistory } from './history-sync'
import type { BackendMessage } from '../types'

describe('mapBackendHistory', () => {
  it('maps bot messages to assistant and user messages to user', () => {
    const rows: BackendMessage[] = [
      { id: 'a', content: 'Hi', is_from_bot: false, timestamp: '2026-01-01T00:00:00Z' },
      { id: 'b', content: 'Hello', is_from_bot: true, timestamp: '2026-01-01T00:01:00Z' },
    ]
    const out = mapBackendHistory(rows)
    expect(out).toHaveLength(2)
    expect(out[0]).toMatchObject({ id: 'a', role: 'user', content: 'Hi' })
    expect(out[0].createdAt).toBeInstanceOf(Date)
    expect(out[1]).toMatchObject({ id: 'b', role: 'assistant', content: 'Hello' })
  })

  it('uses index-based ids when id is missing', () => {
    const rows: BackendMessage[] = [{ content: 'x', is_from_bot: false }]
    const out = mapBackendHistory(rows)
    expect(out[0].id).toBe('history-0')
  })

  it('defaults empty content and missing timestamp', () => {
    const rows: BackendMessage[] = [{ is_from_bot: true }]
    const out = mapBackendHistory(rows)
    expect(out[0].content).toBe('')
    expect(out[0].createdAt).toBeInstanceOf(Date)
  })
})

describe('historiesEqual', () => {
  const base = (): ThreadMessageLike[] => [
    { id: '1', role: 'user', content: 'a' },
    { id: '2', role: 'assistant', content: 'b' },
  ]

  it('returns true for identical id, role, content sequences', () => {
    const a = base()
    const b = base()
    expect(historiesEqual(a, b)).toBe(true)
  })

  it('ignores createdAt differences', () => {
    const a: ThreadMessageLike[] = [
      { id: '1', role: 'user', content: 'a', createdAt: new Date('2026-01-01') },
    ]
    const b: ThreadMessageLike[] = [
      { id: '1', role: 'user', content: 'a', createdAt: new Date('2026-06-01') },
    ]
    expect(historiesEqual(a, b)).toBe(true)
  })

  it('returns false on length mismatch', () => {
    expect(historiesEqual(base(), base().slice(0, 1))).toBe(false)
  })

  it('returns false on id, role, or content mismatch', () => {
    const a = base()
    const idMismatch = base().map((m, i) => (i === 0 ? { ...m, id: 'x' } : m))
    const roleMismatch = base().map((m, i) => (i === 0 ? { ...m, role: 'assistant' as const } : m))
    const contentMismatch = base().map((m, i) => (i === 0 ? { ...m, content: 'z' } : m))
    expect(historiesEqual(a, idMismatch)).toBe(false)
    expect(historiesEqual(a, roleMismatch)).toBe(false)
    expect(historiesEqual(a, contentMismatch)).toBe(false)
  })
})

describe('isHistoryPrepend', () => {
  const base = (): ThreadMessageLike[] => [
    { id: '1', role: 'user', content: 'a' },
    { id: '2', role: 'assistant', content: 'b' },
  ]

  it('returns true when older messages are prepended', () => {
    const prev = base()
    const next: ThreadMessageLike[] = [
      { id: '0', role: 'user', content: 'older' },
      ...prev,
    ]
    expect(isHistoryPrepend(prev, next)).toBe(true)
  })

  it('returns false when length is unchanged or shrinks', () => {
    expect(isHistoryPrepend(base(), base())).toBe(false)
    expect(isHistoryPrepend(base(), base().slice(0, 1))).toBe(false)
  })

  it('returns false when the suffix does not match prev', () => {
    const prev = base()
    const next: ThreadMessageLike[] = [
      { id: '0', role: 'user', content: 'older' },
      { id: '1', role: 'user', content: 'changed' },
      { id: '2', role: 'assistant', content: 'b' },
    ]
    expect(isHistoryPrepend(prev, next)).toBe(false)
  })
})

describe('structured artifact delivery', () => {
  const artifact = { id: 'file-id', name: 'Take [5].mp4', kind: 'video', status: 'active' as const, pinned: false, source: 'generated', source_path: '/source', sha256: 'hash', url: '/api/artifacts/file-id/content', preview_url: '/api/artifacts/file-id/content?preview=true' }
  it('adds a verified link even when the assistant only wrote a summary', () => {
    const [message] = mapBackendHistory([{ content: 'Render completed.', is_from_bot: true, artifacts: [artifact] }])
    expect(message.content).toBe('Render completed.\n\n[Take _5_.mp4](/api/artifacts/file-id/content)')
  })
  it('does not duplicate an existing server link', () => {
    const text = `[Take 5](${artifact.url})`
    const [message] = mapBackendHistory([{ content: text, artifacts: [artifact] }])
    expect(message.content).toBe(text)
  })
})
