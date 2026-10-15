import { describe, expect, it } from 'vitest'
import { personasSnapshotEqual } from './use-ops-poll'
import type { Persona } from '../types'

function persona(partial: Partial<Persona> & Pick<Persona, 'id' | 'name'>): Persona {
  return {
    is_active: false,
    last_bot_message_at: null,
    ...partial,
  }
}

describe('personasSnapshotEqual', () => {
  it('returns true for identical snapshots', () => {
    const a = [persona({ id: 1, name: 'A', is_active: true, last_bot_message_at: '2026-01-01' })]
    const b = [persona({ id: 1, name: 'A', is_active: true, last_bot_message_at: '2026-01-01' })]
    expect(personasSnapshotEqual(a, b)).toBe(true)
  })

  it('returns false when last_bot_message_at changes', () => {
    const a = [persona({ id: 1, name: 'A', last_bot_message_at: '2026-01-01' })]
    const b = [persona({ id: 1, name: 'A', last_bot_message_at: '2026-01-02' })]
    expect(personasSnapshotEqual(a, b)).toBe(false)
  })

  it('treats null and undefined last_bot_message_at as equal', () => {
    const a = [persona({ id: 1, name: 'A', last_bot_message_at: null })]
    const b = [persona({ id: 1, name: 'A', last_bot_message_at: undefined })]
    expect(personasSnapshotEqual(a, b)).toBe(true)
  })

  it('returns false when last_read_at changes', () => {
    const a = [persona({ id: 1, name: 'A', last_read_at: '2026-01-01T00:00:00.000Z' })]
    const b = [persona({ id: 1, name: 'A', last_read_at: '2026-01-02T00:00:00.000Z' })]
    expect(personasSnapshotEqual(a, b)).toBe(false)
  })

  it('refreshes when user or side-chat activity changes without a bot reply', () => {
    const a = [persona({ id: 1, name: 'A', last_message_at: '2026-01-01T00:00:00Z' })]
    const b = [persona({ id: 1, name: 'A', last_message_at: '2026-01-02T00:00:00Z' })]
    expect(personasSnapshotEqual(a, b)).toBe(false)
  })

  it('detects activity-driven reordering and treats absent activity as null', () => {
    const a = persona({ id: 1, name: 'A' })
    const b = persona({ id: 2, name: 'B' })
    expect(personasSnapshotEqual([a, b], [b, a])).toBe(false)
    expect(personasSnapshotEqual([a], [{ ...a, last_message_at: null }])).toBe(true)
  })

  it('returns false when the agent engine changes', () => {
    const a = [persona({ id: 1, name: 'A', agent_engine_effective: 'classic' })]
    const b = [persona({ id: 1, name: 'A', agent_engine_effective: 'cursor' })]
    expect(personasSnapshotEqual(a, b)).toBe(false)
  })

  it('treats null and undefined agent engine as equal', () => {
    const a = [persona({ id: 1, name: 'A', agent_engine_override: null })]
    const b = [persona({ id: 1, name: 'A' })]
    expect(personasSnapshotEqual(a, b)).toBe(true)
  })

  it('returns false when lengths differ', () => {
    expect(personasSnapshotEqual([persona({ id: 1, name: 'A' })], [])).toBe(false)
  })
})
